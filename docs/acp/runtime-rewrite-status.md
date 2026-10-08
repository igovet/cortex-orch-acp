## Runtime orchestration rewrite — implementation and qualification

The 2026-09-21 architecture replaces model-owned orchestration. The runtime owns
lifecycle, dependency scheduling, routes, scopes, permission checks, independent
review, verification, recovery and completion. Models submit bounded proposals and
immutable reports. All 23 specialist/consultant profiles remain available.

Claude ACP support now includes exact 0.87.0 instruction delivery (Claude SDK
0.3.287, ACP SDK 1.7.0, ACP v1), session-observed option handling, and its
non-AIR `claudeCode.toolName=compact` compaction signal. The owner update
path initially found 0.87.0 already installed with SDK 0.3.287 (Haiku 4.5).
A separate exact SDK 0.3.293 refresh now installs beside that original at
`0.87.0+sdk.0.3.293`. Its no-prompt SDK catalog resolves `haiku` to
`claude-haiku-5-5` (Haiku 5.5), lists old Haiku separately, and advertises
low/medium/high/xhigh/max. Actual ACP controls accepted high/xhigh/max without
a prompt. The public preset and owner production `default` profile now
enable the new alias at Luna policy parity and deny old Haiku by exact ID. See [current adapter evidence](claude-agent-acp-adapter.md#current-support-0870-2026-10-07).
These static/no-paid observations do not establish a real model turn or GUI
qualification. Paid live check not requested by the owner.

Codex ACP now includes exact **2.1.1** support and fallback. All 14 guarded
patch sites match 2.0.1; protocol/option mapping is unchanged. The owner update
path found 2.1.1 already installed; checkout discovery now selects it instead
of 2.0.1. Installed CLI 0.159.3 passes three no-paid wrapper/jail checks;
13 fake App Server bundle tests pass against each of 2.1.1 and 2.0.1.
Role metadata now reaches all qualified Codex versions, not only 1.12.0.
No production Paseo/plugin refresh or paid/client qualification occurred.
See [current Codex evidence](codex-acp-adapter.md#current-support-211-2026-10-07).

Implemented production paths:

- Transactional task state, plan revisions, validated DAG, original goal and event
  history in the single project-local store. The scheduler and atomic capacity
  store enforce at most four concurrent workers per chat session across engines;
  per-connection worker limits can only lower that session's share. Coordinator
  reservations keep a separate per-connection limit. Excess ready nodes stay
  pending. Writing nodes with overlapping scopes are serialized; review-stage
  and READ_ONLY nodes do not conflict by scope.
- Closed plan/result/question/scope/replan/escalation/synthesis tools. No public
  delegate, decide, integrate, acceptance or model/effort selection API.
  `result_submit` requires `report` as one plain string: the `report_id` of a published own report or
  the inline Markdown, which the runtime publishes idempotently through the ordinary owned-draft path
  (see [publications](publications-and-consistency.md)).
- Independent review whenever the observed project differs from its execution
  baseline, or for every fix cycle and any unobservable state; unchanged state is
  recorded as `REVIEW_NOT_REQUIRED`. Scoped fixes (targeted to the plan tasks named
  by runtime-checked review findings, otherwise every original task), three review cycles, contained
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
- Worker internal communication remains English on every turn; the coordinator
  receives the selected message language after its context. User-facing worker
  question text and all options are the exception: the advertised question
  contract requires the current turn's `user_question_language` (`en` or `ru`).
  Delivery is tested with scripted models, not automatic translation or a model
  compliance guarantee. Missing project access goes to runtime `access_request`;
  access grants/refusals and bounded writer-conflict retries never ask the user.
  Retained old access cards are retired, not treated as authorization.

## Installed capabilities and the engine jail

The historical 2026-09-30 one-time update added codex-acp 2.0.1 with Codex CLI
0.159.1. At that inspection Claude was 0.84.0 with a 0.78.0 instruction-channel
basis; the current exact registrations are described above.
Earlier inspection verified Codex CLI 0.154.0/0.155.1, codex-acp
1.12.0/1.13.0 and claude-agent-acp 0.78.0 (SDK 0.3.270). The installed hosts expose no
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

## 2026-09-29 optional OpenCode v2 engine

OpenCode v2 (`opencode acp`, any `2.x.x` release) is registered as an optional third engine,
fail-closed. It is onboarded only by `add-opencode-connection`, never discovered
at startup, and excluded from automatic worker routing unless opted in. Launches
are refused without a verified install, the engine jail, unprivileged bubblewrap
process isolation, or a qualified `agentInfo.version`.

`tests/test_engine_opencode.py` runs in the default suite.
`tests/test_engine_opencode_qualification.py`, enabled with
`CORTEX_PINNED_OPENCODE_BINARY`, runs the installed binary through
`EngineSessionManager`, the engine jail, the brokered Cortex MCP server and the
permission bridge, with a loopback fake provider. It observed:

- the Cortex agent as the only mode;
- confirm-or-fail model, effort and mode;
- role and `AGENTS.md` delivery;
- the Cortex gateway reached through Code Mode;
- the in-scope write and the refused out-of-scope write;
- the deletion confirmation card;
- cancellation and usage;
- a sibling jail denied the control server's environment.

The earlier no-paid 2.0.20 pass used an alternate database and extracted logins;
that workaround has been removed. The native-login qualification now passes on
installed 2.0.20 because Cortex removes OpenCode's built-in `opencode.agent` and
`opencode.plan` plugins; ACP offers only `cortex`, the logged-in and custom
providers, and a `/compact` turn completes. No paid or live Paseo turn has
qualified it. Release-specific 2.x compatibility is still not an admission gate.
No paid model or real provider was used by those installed-binary tests. See the
[OpenCode adapter](opencode-adapter.md) for the parity table and limits.

## Verification boundary

The no-paid suite covers real storage, authenticated MCP subprocesses, both engine
protocol paths, complete plan → worker → reviewer → verification → synthesis runs,
Linux namespaces, cancellation/recovery races and transcript retention. Model
sessions are scripted unless a test explicitly states otherwise. Historical baseline (2026-09-21): the Python suite passed **2,354 tests with no skips** (146.92 seconds), including installed adapter and CLI checks. TypeScript and **16 interface tests** pass; the final
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


## 2026-09-25 runtime and plugin remediation

The subsequent remediation added strict per-check evidence, retained check history with exact later-run closure, and transport recovery separated from task outcomes. Passed checks require a same-task, same-attempt `command_run` or user-confirmed `outside_command` receipt for a completed nontrivial command, exit 0 and complete retained output. Weak or missing evidence stays unverified; old reports are not rewritten. A later verified rerun closes a failure only when it is the latest run of that exact command. See [ACP orchestration](orchestration.md#check-evidence) for the contract.

Disconnect handling persists `host_disconnected` with available session/task/attempt identities, interrupted workers, open command receipts, runtime package version and history completeness. Restart recovery reads saved state and does not automatically repeat actions whose outcomes are unknown. A separate engine-launch failure diagnostic includes stage, task/session/attempt identities and the running package version; the diagnostic command reports the installed runtime version separately from the checkout. A regression test covers the saved question → Russian answer → process restart cycle and asserts one answer with no repeated question.

The Paseo plugin now derives interrupted-card display from the retained worker state and uses a title suffix because the installed ACP bridge drops `_meta` on `tool_call_update`. GUI RPC handlers use asynchronous subprocess execution, bounded concurrency and cached identities; overlapping worker-history and pipeline requests are coalesced, and invoke timeouts identify the method. These claims describe implemented surfaces; a passing automated suite or plugin test alone does not establish every rendered GUI behavior.

Until reload recovery is qualified for the deployed version, apply plugin updates between active tasks. Remaining limits: a host `SIGKILL` without `SIGTERM` cannot be recorded, and native engine actions are not recordable as complete command receipts. The plugin CLI tests and final no-paid suite are listed in [verification guidance](../project/verification.md); report exact run outcomes for the checkout being qualified.

## 2026-09-26 live GUI qualification attempt — blocked before scenario execution

An attempt to run the four planned isolated-dev-Paseo scenarios (strict evidence acceptance, plugin/daemon-restart resilience, question-answer restart, cold-open history) reached environment setup but could not execute any scenario, because no tool available in this worker session can attach to or observe the Paseo Electron app's rendered surface: the session's Playwright MCP instance is bound through a browser-extension relay to a separate Chrome browser, not connectable to the isolated app's own remote-debugging endpoint, and no computer-use/VNC tool targets the private on-demand Xvfb display the guarded `gui --xvfb` wrapper creates for that app. `docs/acp/dev-paseo-environment.md` already states CLI output, logs and CDP inspection alone do not establish a rendered GUI result, so this is a genuine capability gap, not a policy substitution.

Verified before the blocker (all commands ran from this checkout with retained exit-0 receipts): native Codex and Claude login status (both logged in, no credentials printed); `integrations/paseo` `npm ci`, `typecheck`, `test:activation` and `test:steer` all passed; the guarded wrapper's `--check` selected dev home `/home/igovet/.paseo-dev` and named `/home/igovet/.paseo` as protected production; the isolated dev daemon started on `127.0.0.1:16767`; `plugin ls` confirmed the `cortex-acp` plugin already registered against this exact worktree path, enabled and `running`, so no reinstall was needed; a disposable project was created outside the checkout. The `gui --xvfb` launch produced a real Electron process (verified by start-time correlation against an unrelated, much older production Paseo process, which was left untouched) and a DevTools listener, but with no way to drive or read its rendered content the session was torn down cleanly: the dev-only Electron process and its orphaned private Xvfb child were stopped, and the dev daemon was stopped, leaving no daemon or GUI process behind. Production `~/.paseo` and the user's own Zed/desktop session were not touched.

All four scenarios remain unrun. Completing them needs a tool in a future worker session that can either attach a browser-automation tool to an arbitrary external CDP endpoint (the isolated app's own remote-debugging port) or drive/screenshot the specific private Xvfb display the guarded wrapper allocates.

## 2026-09-26 live GUI qualification, retried — CDP-screenshot method reused, three of five scenarios observed, one blocked by a live account error

A later worker session closed the prior capability gap: connecting a plain Python `websocket-client` directly to the DevTools URL the guarded `gui --xvfb` wrapper prints (`suppress_origin=True` avoids the CDP origin check), calling `Page.captureScreenshot`, and visually inspecting the returned PNG is a real rendered-GUI observation, not "CDP inspection alone" — it is the same method the 2026-09-25 remediation attempt used for its own rendered check. `Page.navigate` to a known `paseo://app/h/<server>/workspace/<id>` URL and raw `Input.dispatchMouseEvent`/`dispatchKeyEvent` calls drove navigation, clicks and a multi-choice-question answer inside the isolated app; native OS file-picker dialogs opened by "Add project → Browse" could not be driven this way and were abandoned in favor of `paseo run --cwd <disposable path>`, which attaches a new workspace without needing the picker.

Setup, all against `~/.paseo-dev` / `~/.config/Paseo-dev` / daemon `127.0.0.1:16767`, plugin already registered at this exact worktree path (enabled, `running`, no reinstall needed): `npm ci`, `typecheck`, `test:activation`, `test:steer` passed; native Codex login confirmed logged in without printing credentials; disposable project created at `~/paseo-dev-disposable-projects/qa-debugger-20260926` (git-initialized, outside the checkout).

Observed scenarios:

- **(a) strict evidence acceptance — confirmed.** A real `cortex/gpt-5.6-luna` task ("add `negate(a)`, add a pytest case, run pytest") reached `Runtime: COMPLETED` in ~2 minutes. The rendered reviewer WORKER card's own text stated it independently reran `pytest` via `command_run` rather than trusting the implementer's claim, both tests passed exit 0, and the card cited a named check (`pytest test_math_ops.py (independent rerun)`) rather than an unlinked "passed". This is the positive strict-evidence case; a weak/uncited-pass counter-case was not separately constructed live (would need deliberately forcing a check without a command receipt) and is not claimed as tested.
- **(e) Workers button/popover removed — confirmed.** Every screenshot's composer showed only a `Pipeline` pill; no `Workers` pill or popover appeared anywhere across roughly a dozen screenshots of different chats, consistent with the 2026-09-26 decision to remove it.
- **(b) plugin-reload resilience — partially confirmed, one part inconclusive.** With the scenario-(a) task already `COMPLETED` and a second task's implementer worker actively running, `plugin reload cortex-acp` was issued (the same event a plugin update produces). After reload, the completed task's rendered state stayed `Runtime: COMPLETED` — it was **not** shown as failed — but the host's own reconnect text is rendered directly appended with no separator (`Runtime: COMPLETED[System Error] Provider connection closed`), a cosmetic artifact distinct from the state itself being correct. The interrupted worker's card did not visibly flip to any distinct "interrupted" marker on reload or on a full page reload; it stayed rendered as a stale `Working` pill until later chat activity superseded it. A live, unrelated account error (below) started during the same window, so whether a clean "Interrupted" render exists for this exact build could not be isolated with confidence; this part is reported as inconclusive rather than pass or fail.
- **(c) question-answer restart — blocked by a live account error, not by reproduced product behavior.** A genuine coordinator fallback question ("Continue" / "Finish: accept the current results and close the task") appeared live. "Finish" was selected (visually confirmed selected) and submitted in the rendered UI, but the coordinator's next turn, and every subsequent retry, failed with a real `401 Unauthorized: Incorrect API key provided` from the Codex-backed backend route (independently reproduced with a trivial unrelated one-line task on the same route), so the same fallback question kept reappearing. This reproduces the visible symptom named in the brief but the concrete, disclosed blocker is a live backend-API-key/account failure outside this worker's scope to fix (native login was checked and reported logged in before the session; the key evidently stopped being accepted partway through). This check is `unverified`, not `failed`: the "Finish" answer was never actually processed by a working coordinator turn.
- **(d) cold-open of long history — partially confirmed.** An existing, real chat with substantial history (a large diff, unrelated unaffected project, part of the documented full clone of production Paseo state used for realistic dev testing) rendered in ≈1.6 seconds on first click with no visible stuck spinner and no per-item permission prompt. The isolated dev daemon's own log recorded zero `cortex-acp.invoke` and zero `timeout` occurrences for the whole session. However, that specific chat used a direct, non-Cortex provider with no worker cards, so the "many worker cards" condition in the brief was not exercised before the account error above stopped further live coordinator turns; no stronger candidate chat was tried.

No message bodies, screenshots of private content, or raw daemon/plugin log lines are recorded here or elsewhere in the repository; only exit statuses, counts, identities and safe structural summaries are retained, per policy.

Cleanup: the two agents left in a stuck state by the account error were stopped via the guarded CLI; the dev-only Electron process and its private Xvfb were terminated by PID after start-time correlation against the unrelated, much older production Paseo process (left running and unmodified — confirmed still running under its original PID afterward); the isolated dev daemon was stopped. Production `~/.paseo`, its daemon, the production Cortex agent and the user's own Zed/desktop session were not touched at any point.

**Next action.** Re-run scenario (c) and complete scenario (b)'s inconclusive half and scenario (d)'s multi-worker-card case once the coordinator's backend API key/account is confirmed healthy again; construct a deliberate weak/uncited-evidence case for scenario (a)'s negative half.

## 2026-09-26 the live Codex `401 Incorrect API key` was an upstream OpenAI incident, not a local key

The `401 Unauthorized: Incorrect API key provided` that blocked scenario (c) above came from no API key on this machine and from no Cortex defect. What the evidence shows (names, booleans and identities only; no credential value was read into any artifact):

- **Route.** Every failing call went to `https://chatgpt.com/backend-api/codex/responses` on the dev Cortex's only Codex connection, `cn_8e5a7956b25d` (`auth_mode=native_account`, `credential_ref` = the user's own `~/.codex`, no secret file). The coordinator (`gpt-5.6-luna`) and Codex reviewers (`gpt-5.6-terra`, `gpt-6-sol`) all failed the same way. The disposable project's `attempt_route_audit` rows bind each attempt to that connection. Separately, the user's `~/.config/cortex` has one `api_key` Codex connection (`cn_a3e9d1e99a33`, "work"), but it is in `authorization_required` state, has no secret file, and was not used by the dev runtime, which reads only `~/.paseo-dev/cortex-xdg`.
- **No local key.** `~/.codex/auth.json` is `auth_mode=chatgpt` with `OPENAI_API_KEY: null`, and `~/.codex/.env` does not exist. The only `model_providers` entry in `~/.codex/config.toml` is an unrelated non-OpenAI provider, and `model_provider=openai`. Codex's own request tags for the failing processes report `auth_mode="Chatgpt"`, `auth_env_openai_api_key_present=false` and `auth_env_codex_api_key_present=false`. That matches `connections.environment.build_process_env`, which injects a key variable only for `api_key` connections.
- **Rejected on OpenAI's side.** From 22:39:37Z on 2026-09-25, every Codex process's model call was rejected. Before that, calls over WebSocket had succeeded. Each ChatGPT token refresh returned 200, and `/models` and analytics calls with the same bearer also returned 200. The 401 response itself carried the account's real plan headers (`x-codex-plan-type`, `x-codex-primary-used-percent=89`, quota not reached). The credential it echoed back as rejected is a masked OpenAI *service-account* key (`sk-svcac…`) that exists nowhere on this machine.
- **Not Cortex.** Dev Paseo's own native `codex/gpt-5.6-luna` provider, with Cortex not involved at all, failed identically at 23:19Z and again at 23:23Z. Many users on other machines and operating systems reported the same echoed key from 22:48Z (openai/codex issues #48230, #48232, #48255–#48265, several still open at 23:22Z).

Why Cortex showed "the coordinator did not produce a plan or an answer" (`structured_proposal_missing`) rather than an authentication cause: Codex reported this failure as a retried stream disconnect (`responses_retry`, five retries per turn), not as an `unauthorized` or HTTP-401 error. Consistent with that, the Cortex turn ended normally with only the error text, and no JSON-RPC error reached Cortex. codex-acp 1.13.0 raises one only for `usageLimitExceeded` or an `unauthorized`/HTTP-401 `codexErrorInfo`; the exact `codexErrorInfo` value sent was not captured. `engine.failure_classification.route_failure` deliberately trusts only structured engine fields, never engine prose, so no route-unavailability record could be made. This is a diagnosability limitation of the current engine signal, not a misclassification.

**Owner action.** None locally. Do not create an API key, log out or log in again: the ChatGPT login is valid and was accepted. Live checks that need a Codex route must wait until OpenAI resolves the incident (a single native-Codex prompt in the dev Paseo succeeding is the discriminating check). Until then, a Claude route can be used for live coordinator turns. Scenarios (b), (c) and (d) above stay unverified until a Codex or Claude coordinator turn runs cleanly.

## 2026-09-26 live GUI re-qualification — 401 resolved, (a2) and (c) confirmed, (d2) confirmed, (b2) not confirmed (stale card, no duplicate mutation)

The prior Codex 401 incident is over: a trivial one-line `cortex/gpt-5.6-luna` task in a fresh isolated-dev-Paseo disposable project completed end to end with a real answer, confirming the coordinator route works again. Setup matched policy throughout: `~/.paseo-dev` daemon on `127.0.0.1:16767` (started via `outside_command`, not the sandboxed in-checkout runner — a daemon started under the sandboxed runner cannot write `~/.codex` or `~/.config/Paseo-dev` and its native Codex child fails with `failed to initialize sqlite state runtime`, matching the class of P1 item 4 in the 2026-09-25 audit), `~/.config/Paseo-dev` Electron profile via `gui --xvfb`, plugin already registered at this exact worktree path, package `validate`, the full no-paid suite (3226 passed, 13 skipped) and the plugin's `npm ci`/`typecheck`/`test:activation`/`test:steer` all green beforehand. A disposable project outside the checkout was used throughout; production `~/.paseo` (`~/.config/Paseo`, PID unchanged throughout) was never started, stopped, or reconfigured. The dev daemon was stopped at the end.

- **(a2) weak/uncited-evidence claim renders unverified — confirmed.** A task instructed a worker to mark a check `passed` using only a bare `pwd` command as evidence for a file-content claim. The worker itself declined and used a real byte-level check instead; forcing the point further, the coordinator's own final synthesis explicitly listed under "Not verified": *"result.txt contains VERIFIED — passed with only bare pwd evidence — blocker: A bare pwd command reports the working directory and cannot establish result.txt contents; the runtime evidence policy disqualifies it as proof for a content check."* The genuinely-passing byte-level check was recorded separately and correctly. This is real evidence of the P0 strict-evidence acceptance rule holding, reproduced independently a second time later in the same session on an unrelated task's own check.
- **(d2) cold open with many worker cards — confirmed.** A single task with five independent zero-dependency worker nodes (plus one reviewer) all reached `Completed`; a full Electron page reload (`Page.reload`) then cold-opened directly back into that six-card chat, rendering the entire timeline correctly with no per-card confirmation prompts. The dev daemon log gained 68 lines across the reload with zero occurrences of `timeout` and zero occurrences of `cortex-acp.invoke`.
- **(c) answered question, daemon restart, question not re-asked — confirmed.** A real coordinator fallback question (repeated `structured_proposal_missing` from live Codex-route rate-limiting, HTTP 429, not a reproduction of the original bug's specific cause) was answered with free-text "Завершить". Before that answer could be processed, the dev daemon was stopped and restarted, and the Electron page was reloaded. The reopened chat showed exactly one new system line — *"Cortex restarted after the host connection closed. Any connection error concerns the transport only; the saved task state is authoritative: WAITING_USER."* — with no new duplicate "Choose an answer" card rendered and no automatic retried mutation.
- **(b2) plugin reload with an active worker — not confirmed.** A worker was confirmed actively `Working` (real `sleep 40` mid-flight), then `paseo plugin reload cortex-acp` was issued. The coordinator's own connection dropped (`[System Error] Provider connection closed`), but the worker's card kept rendering a stale green `Working` pill, unchanged, through both the reload and a subsequent full page reload — no `host_disconnected` recording or "Interrupted" title-suffix ever appeared for it. Only after a fresh user message ("continue") did the coordinator notice the deliverable was missing, cancel the stale assignment, and dispatch exactly one fresh retry, which completed and was independently byte-verified — so **no duplicate mutation occurred** — but the stale card's terminal display was a generic `Failed`, not the distinct `Interrupted` status the 2026-09-25 fix (see the "Runtime and plugin remediation" section above) intends, and it never updated on its own from a plugin reload or page reload alone. This CLI `plugin reload` may not be the same code path as the SIGTERM-then-SIGKILL plugin-update sequence the fix targets; this is reported as a concrete, disclosed gap, not a pass.

Checks not run: a genuinely clean SIGTERM/SIGKILL-driven plugin-update reload (only the CLI `plugin reload` subcommand and a full page reload were exercised) — blocker: no supported wrapper command performs that exact host-level sequence against the isolated dev daemon from this worker's tools. No message bodies, screenshots of private content, or raw daemon/plugin log lines are recorded here; only exit statuses, counts, identities and safe structural summaries are retained.

**Next action.** Investigate whether the Paseo plugin's own reconciliation path is reachable from a `plugin reload` (Node-process-level restart without an explicit `host_disconnected` emission) the same way it is from a full daemon/process restart, and if not, extend the disconnect-detection to cover it.

## 2026-09-26 plugin reload with an active worker — fixed and confirmed live

Root cause of the (b2) gap above, from retained evidence: the old process did record `host_disconnected` on `plugin reload` (the earlier run's record was written 7 ms after Paseo logged `Provider connection closed`). The display failed for three separate reasons. Paseo 0.9.2 does not reload the session after a plugin reload, so the `session/load` correction never ran before the next prompt. The Paseo card trusted only the host's stale `running` status. And the client parsed a status suffix only inside the route, while the runtime appends it to the whole title, so even a delivered `· Interrupted` or `· Cancelled` read as a generic failure. After the next prompt, recovery's `recovery_worker_lost` and the replan cancellation rendered as `Failed`, and the replacement process re-sent the card as a second row.

Changes: the card polls the durable attempt status while the host shows it running and settles on it; the parser accepts the trailing suffix; `recovery_worker_lost` (and the cancellation after it) projects as Interrupted on the card, the Pipeline panel and `session/load` replay, with a localized reason; the recovery turn sends no update for a lost card the host already shows; each worker launch records its owning Cortex process, so a replacement no signal reached is identified read-only; and `host_disconnected` records its `trigger`.

Live, isolated dev Paseo 0.9.2 (`~/.paseo-dev`, `~/.config/Paseo-dev` via `gui --xvfb`, plugin at this checkout, disposable project outside it, `gpt-5.6-luna` medium), three runs, each with a worker in `sleep 150`, one `plugin reload cortex-acp`, one full page reload and one follow-up message. Rendered state was captured with CDP `Page.captureScreenshot` plus the card's rendered text:

- Before the reload the card read `Working`. Right after the plugin reload, and again after the page reload, it read `Interrupted`, with no new message. The Pipeline step used the Interrupted icon and showed the Russian reason in the Russian-language task.
- The old process received SIGTERM (`trigger: sigterm`). It persisted `host_disconnected` about 0.4 s after the reload started, marked its engine processes terminated and exited within about 0.1 s, with no SIGKILL needed.
- After the follow-up message, recovery wrote `recovery_worker_lost` and the replan cancelled the assignment. The lost card still read `Interrupted`, and one retry ran on a new card. The interrupted `command_run` receipt stayed `complete: false` and was never re-run; the coordinator checked that the target file was absent before re-delegating; the file was written exactly once.
- Runs 1 and 2 (processes started before the last change) showed the lost worker twice after the follow-up. Run 3, with the final code, showed exactly one Interrupted row and one new retry card.

Not verified live: a real SIGKILL-without-SIGTERM replacement. It is covered by an automated test that uses a genuinely SIGKILLed process as the owner. The deployed production package does not contain this change, so keep applying plugin updates between active tasks there. No message bodies or raw log lines are recorded here.

## Settings-only model routing and saved profile management

The current router and coordinator no longer classify model names/families or use position-derived tiers, name-based exclusions, model seeds, or silent template fallback. Effective routing requires explicit version-2 owner policy and currently observed catalogs; missing, invalid, unsupported, ambiguous or unavailable settings fail visibly. Automatic membership and enabled identities are explicit lists; omitted values select nothing. Tiers and non-neutral effort/work-kind/request-label policies are per exact engine/model identity. The coordinator's portable preference binds only to exactly one current selectable engine/model/parameter match. Generic role/complexity scheduling remains; names never grant eligibility. See [configuration](../configuration.md#settings-only-model-policy).

policy/model-settings-default.json is public install/migration data only. Fresh installation validates it and creates only missing owner files, with private no-replacement writes. Existing files and profiles are never silently overwritten or migrated. Existing settings require explicit, digest-bound preview/apply; saved profiles migrate only from their own candidates. Automated tests imply no production adoption or live route qualification. The former complete core run's profile integration failure was historically recorded; its dependent follow-up reports unfiltered 396-case core and 428-case targeted checks passing.

Saved-profile deletion asks confirmation for the exact name and uses the last observed revision. Cancel is mutation-free. Selected deletion clears only saved-profile linkage and leaves materialized settings unchanged; deleting the last profile is valid. Refusals/errors preserve profile and settings; stale/error paths do not retry invisibly. Typed plugin checks establish form/model/API interaction, not installed desktop reachability or rendered GUI qualification.


## 2026-10-05 orchestration measurement, overlap and dev wire tracing

The checkout now includes metadata-only critical-path observations:
`COORDINATOR_TURN_STARTED`, per-attempt `ENGINE_READY` and
`FIRST_MODEL_OUTPUT` events, plus change-only scheduler wait diagnostics
(dependency, scope overlap, session/connection capacity and route capacity).
`scripts/cortex_timeline.py` reads a checkpointed project store without
initializing or mutating it and reports exact observed phase pairs; missing,
duplicate, invalid or out-of-order endpoints remain explicit, and overlapping
worker intervals are not summed into a fabricated total. It refuses legacy or
unsupported formats, busy/uncheckpointed storage, missing tasks and event sets
over its cap.

Accepted execution results can trigger independent review before their engine
processes fully exit. Accepted workers lose channel authority immediately, but
their capacity slot remains occupied until exit is confirmed. Review remains
digest-bound and late project drift requires rebinding/review. Per-file digest
hashes are reused only on recognized local filesystems when the observed
metadata key is unchanged; manifest identity and drift gates are unchanged.

One sandboxed verification sequence may run while the capacity-checked reviewer
is active. The candidate is bound to the plan, reviewer and review digest; only
an unchanged APPROVE can credit it. Stale, rejected, interrupted or failed
candidates do not become passes; approval waits for worker teardown and ordinary
verification/recovery rules still apply. No faster live completion or measured
speedup is claimed.

Opt-in wire tracing is available only on guarded `~/.paseo-dev` daemon start.
The wrapper strips inherited opt-in for ordinary invocations; installed
production state/payloads are refused. Redacted ACP (upstream/downstream), MCP,
verification and lifecycle records use private 0700 directories/0600 files,
four 16 MiB rotating files, and remain outside project stores and Git. Redaction
and write failures are fail-safe for task behavior but can leave explicit trace
gaps. Traces are sensitive diagnostics, not command receipts, commit receipts,
acceptance evidence or a substitute for retained task events. The read-only
`scripts/cortex_trace_view.py` filters existing JSONL but can exit on unsafe,
invalid or incomplete files; stop the writer for exact analysis.

Engine prewarming (proposal item 4) remains deferred until a permitted live
measurement separates process/ACP startup from model latency. These implemented
surfaces and automated tests do not release-qualify the runtime or prove an
end-to-end performance improvement.


## 2026-10-05 routing defaults and trace boundary follow-up

An explicit user-requested model is the highest routing preference across tiers
and retries, but it does not bypass authorization, current catalog identity,
supported effort or engine availability. If that exact route is unavailable or
unauthorized, the runtime exposes a hard refusal rather than substituting a
model. Usage-based ranking becomes active only at observed usage >=95%; below
that threshold it is neutral, and missing/unknown observations remain
unavailable. See [routing and fallback](routing-and-fallback.md).

Fresh-install settings now import the owner's saved default as portable
installation data: routing policy, coordinator engine/model/parameters and
message language. This is not a runtime fallback or a full profile clone.
Existing owner files are never replaced; local account/connection bindings and
machine-specific MCP definitions and paths are excluded. See
[configuration](../configuration.md#fresh-install-owner-default-snapshot).

The developer trace writer and viewer now open files non-blockingly and
close-on-exec, while retaining no-follow and descriptor checks that accept only
safe regular files. FIFO paths therefore cannot hang either operation; unsafe
files are refused without being altered. This hardens the existing opt-in
`~/.paseo-dev`-only tracing boundary; it does not enable tracing in production
or make traces complete, nonsensitive, or acceptance evidence.
