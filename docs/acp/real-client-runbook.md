# Real-client observation runbook (Zed, Paseo, codeg)

This runbook is the reproducible GUI observation list for qualifying Cortex
against real ACP clients. Zed is the current target; codeg is deferred.
The 2026-09-19 isolated Zed 1.20.2 pass covers startup, command fallback,
no-connection behavior, onboarding repair and no-paid structured history
replay after restart. A later [real ACP baseline](evidence/zed-2026-09-20/live-acp-baseline.md)
adds direct-project worker execution, autonomous completion and real worker-card
replay, while also recording failures. Unobserved rows remain `NOT RUN` or
`BLOCKED`; the baseline does not qualify a later source revision. See also
[the earlier observation record](evidence/zed-2026-09-19/observations.md).

Mock/harness output (`scripts/cortex_acp_smoke.py`, the pytest ACP suite)
is real automated evidence for the protocol/launch surface, but it is
**not** GUI evidence and must never be cited as satisfying a row below.
The current candidate also needs real-client observations of direct native
engine execution across both coordinator/worker engine pairings, plan references,
revise continuation, failure recovery and cancellation cleanup. A
[real counter repair task](evidence/zed-2026-09-19/live-counter-failure.md)
was attempted after isolated Codex authorization and **failed**: the workers
did not launch and the final answer claimed an unintegrated fix. This old
failure remains a failure; the 2026-09-20 baseline later observed successful
direct-project workers. The standalone paid
engine probes were not run; codeg remains deferred. A separate
[Paseo production-client record](evidence/paseo-2026-09-20/live-acp-regressions.md)
contains the completed read-only worker task, its incorrect final hash,
and the subsequent source correction that still needs a fresh GUI run.

The runtime rewrite’s latest isolated Paseo observations, including retained
failures and successful Luna/Haiku completion, are recorded in the
[2026-09-21 runtime evidence](evidence/paseo-2026-09-21/runtime-rewrite.md).
The [2026-09-20 final evidence note](evidence/paseo-2026-09-20/final-observations.md)
is historical and does not qualify the rewrite.
They cover a bare `/harvest` task, a bare `/harvest-refresh` task, native
Paseo persistence replay, and a no-paid cancellation fixture. The two
harvest tasks were accepted by the durable runtime; the refresh run still
had a stale wait reason in its final UI and therefore does not qualify clean
Paseo completion. The native persistence result was verified through the
Paseo CLI after reload, but a full GUI restart observation remains unrun.
The fixture cancellation replay is separate from live-client qualification.
Other Paseo rows remain `NOT RUN` unless an evidence note explicitly records
an observed result.

A later [direct-project retry](evidence/zed-2026-09-19/live-direct-project-failure.md)
also **failed**: the coordinator selected a policy-permitted Claude route that
was not connected in the isolated profile. The user stopped that run; the
project-root test still failed. The retry showed that a Codex worker can start
with the Zed project as its working directory, but it did not complete the
repair and left temporary engine metadata in that directory.

The next qualification uses the same disposable Zed profile and project,
but Cortex now references the user's existing production Codex and Claude
native accounts. Only the Zed profile and Cortex XDG configuration/state
are disposable. The two failed runs above remain failures; changing the
account wiring does not retroactively qualify either run.
The current landing qualification retains seven failed runs. The seventh
read-only QA run used `from math import pow` in a Bash heredoc and still
required manual permission. The worker Bash classifier now auto-approves
non-destructive observed commands and `>` redirects whose targets are all
inside the bound workspace and outside `.cortex`; `/tmp`, private targets and
deletion remain host-confirmed, while dynamic or opaque interpreter effects
remain unclassifiable. The targeted permission probe then passed the exact
Bash auto-approval and final acceptance checks, but the clean MCP criterion
failed: the first `publish_result(report_id)` used invalid arguments, and one
legitimate late `decide(accept)` meaningful event was refused. The schema and
worker template now map `write_report.report_id` to `publish_result.report_ref`;
the runtime ends the turn after an exact pending-event refusal and delivers
that event before a retry. This does not guarantee that the first refusal can
always be prevented.

The eighth landing run also failed qualification: a visible-focus claim was
accepted without rendered evidence. QA rendered-evidence checks now require
the dedicated QA profile rule and retained rendered output; that claim is not
credited as a live pass. The ninth landing run delivered 16 successful Cortex
MCP calls and both reports were read by the coordinator, but QA did not read
the predecessor report and the coordinator's final Bash command required host
permission; no final decision was recorded. A fake-wire `allow_once` probe for
that exact command emitted zero host requests, while an `rm` probe still
requested host approval. A new prompt handoff now names
the exact predecessor `report_ref` and requires its trusted `read_report`
receipt. The coordinator permission rule is now documented above. A tenth
landing run was incomplete: normal read-only coordinator actions produced no
prompt, and the Codex rendered check passed after an initial socket failure.
Worker `rm` cleanup correctly triggered a Paseo permission request; it was not
approved, the files remained, and the run stopped with no report, QA result, or
decision. Zero MCP errors were observed, so the deletion guard passed, but the
overall run is not a qualification pass. An eleventh landing run is pending.
The eleventh landing run also failed: worker static tests passed, but rendered
evidence remained honestly `UNVERIFIED` after bounded retries. Redundant npm
and Chrome attempts triggered the bounded verification-prompt rule. The report
was published cleanly on its first call, but the coordinator did not read it;
an ordinary `pytest | tail` then caused a false permission prompt and stopped
the run. No MCP errors or approval occurred, and there was no QA result or
decision. The exact predecessor `report_ref`/trusted `read_report` handoff
rule therefore remains unexercised because the run stopped. The twelfth
landing run then passed bounded worker behavior with one rendered attempt and
no redundant fallbacks; the frontend report was published and the exact
coordinator read succeeded. A safe `pytest | tail` was still false-prompted;
there was no approval, QA result, or decision, and zero MCP errors. The unified
execute/other bounded classifier now treats `2>&1` as safe where the rest of
the command is safe; true truncation, deletion, private, and outside targets
remain reviewable. Independent fake-wire checks for both shapes passed (58
focused checks). The thirteenth landing run failed immediately on benign
test-reading loop syntax under the former narrow allowlist; no delegation or
report occurred. The policy is moving to broad default-allow for declared Bash
with destructive/private/outside/malformed guards, while opaque interpreter
effects remain limited. The rawInput/locations path gap fix is now in place,
with outside/private/symlink guards and hard worker refusal; bound targets are
auto-approved subject to the same lexical limits. The fourteenth landing run
was accepted by the durable state but failed qualification: one MCP error
omitted `assignment_ref` from `decide`, there was no QA, duplicate rendered
checking wasted work, and the pipeline showed Claude Sonnet medium while the
actual route was Codex Luna high. No permission was requested; the exact
report read and coordinator rendered check passed, but the bad-control check
failed. Fixes include the decide schema, same-handle worker rule, and an
independent-review default for visible UI with a concrete recorded low-risk
exception and exact durable route/ID copying; these fixes are complete. A
fifteenth landing run is pending. The fifteenth run failed the role boundary:
the coordinator directly wrote `index/styles` without delegation or
permission, and the accepted broad edit policy was incorrect. The final rule
refuses coordinator source edits locally with a notifier, allows only the
exact owner-bound pipeline draft, allows worker edits inside their workspace,
and keeps deletion host-reviewed. There was no QA or decision. A sixteenth
landing run is pending. Do not claim complete deletion enforcement, native
Codex compaction
persistence, or full live qualification until retained real-client evidence
exists.

The twentieth landing observation also **failed qualification**: the worker's
structured rendered-check result was missing or unverified while its report
prose was treated as sufficient for assignment acceptance. The retained
regression reproduction is
[`tests/test_delegation_mcp_tools.py`](../../tests/test_delegation_mcp_tools.py);
it records the exact acceptance invariant without copying any private live
transcript. A report cannot certify a current work-graph `rendered_ui`
requirement without an observed structured check, and this case must not be
counted as a pass.

The twenty-first landing observation **failed hard** when the Claude worker
read a private `.cortex` pipeline file through a native file tool instead of
the bounded Cortex report route. The retained diagnostic is
`/var/tmp/cortex-final-clean-git-6fs6hc7f/qualification-failure.md` and the
corresponding UI observation is
`/var/tmp/cortex-final-clean-git-6fs6hc7f/final17-ui-verification.md`. These
paths identify the evidence location only; the repository does not copy the
private file contents. No task decision or independent QA result from this
run is accepted as qualification evidence.

The role-refusal repair now records a meaningful
`coordinator_role_refusal` event when native Claude returns its generic
`reject_once` result, suppresses misleading refusal prose, and continues the
same coordinator session with delegate guidance. The composed/offline path is
covered, but a fresh live Claude observation is still `NOT RUN`.

The [twenty-second live qualification](evidence/paseo-2026-09-21/live-run-22.md)
correctly exercised revision: the revised candidate was produced and its
exact candidate check passed. It is nevertheless **FAIL**, because the run
recorded no final task-level decision and no independent Claude QA result.
The passing revised candidate does not qualify the orchestration by itself.
The next run must retain the revised evidence, obtain independent QA, publish
the current pipeline edition, and record the explicit overall task decision.

The [twenty-third Paseo qualification](evidence/paseo-2026-09-21/live-run-23.md)
also **FAIL**. A native worker helper targeted an outside-project `/tmp` path,
and the observation contained zero ACP permission requests. The absence of an
ACP request is not permission or OS containment evidence, so the run cannot
qualify the boundary. The current Codex temporary-tree exclusion request has
offline/request-trace coverage only; its live effect remains `NOT RUN` until a
fresh real-client probe records it.

The subsequent [clean ACP boundary probe](evidence/paseo-2026-09-21/live-boundary-probe.md)
used a disposable Git project and a real Codex Luna medium worker. Its exact
`workspaceWrite` request included empty `writableRoots`, disabled network,
`excludeTmpdirEnvVar`, and `excludeSlashTmp`. The worker created the requested
`index.html`; its single native `/tmp` sentinel attempt failed with
`Read-only file system`, and the sentinel was absent. Record this as **PASS
for the observed `/tmp` exclusion outcome only**. It does not qualify
`/var/tmp`, `.cortex`, arbitrary same-user isolation, or full task completion;
those remain `UNVERIFIED`.

The following Paseo landing attempt also **failed qualification**. Repeated
local refusals interrupted coordinator-side project verification before the
independent Claude QA worker published its review. It therefore has no
complete coordinator/QA/completion chain and is not a pass. The refusal
continuation repair has composed/offline coverage; a fresh real Claude
observation is still required.

The [twenty-fifth Paseo qualification](evidence/paseo-2026-09-21/live-run-25.md)
also **failed qualification**. The Codex implementation worker's rendered
check was unverified because the native browser checker hit a host crashpad
restriction, and its parallel nested command was absent from the ACP worker
history. An independent Claude QA worker later observed a passing rendered
check, but its publication had no structured `worker_result.checks` entries.
Bound candidate checks observed both rendered and static passes, yet the
acceptance gate correctly refused the implementation, QA and task decisions;
the task ended stopped with no durable task decision. This run does not
qualify completion, and the next run must retain structured QA evidence and
project all nested parallel commands.

The [twenty-sixth Paseo qualification](evidence/paseo-2026-09-21/live-run-26.md)
also **failed at first delegation**. The coordinator supplied the optional
`plan_assignment_ref` as an empty string, which the advertised MCP schema
correctly rejected. A retry omitted the field and provisioned a worker, but
the run was stopped before implementation, QA, publication or any task
decision. This remains a live qualification failure; the next run must omit
unset optional fields at request construction and repeat the complete task in
a fresh disposable project.

The [twenty-seventh Paseo qualification](evidence/paseo-2026-09-21/live-run-27.md)
also **failed**. The Codex worker completed project edits and a static check,
then ended its native turn normally without publishing the required durable
report or structured result. Cortex correctly recorded
`worker_ended_without_result`; no independent Claude QA, candidate acceptance,
final pipeline decision, or task acceptance followed. The run predates the
same-session corrective continuation and provides no live evidence for that
recovery behavior. A fresh disposable run must verify the bounded continuation
and its truthful second-miss failure path.

The [twenty-eighth Paseo qualification](evidence/paseo-2026-09-21/live-run-28.md)
also **failed**. Both implementation and QA workers published reports and the
independent project checker passed all eight static checks, but the coordinator
first called `read_report` with `report_ref` while that MCP contract requires
`report_id`. The retry succeeded, but the first call is a real MCP error and
violates the zero-MCP-error criterion. QA's rendered-browser/accessibility
check was `not_run`, no task-level decision was recorded, and same-session
missing-publication recovery was not exercised. The run therefore provides no
live completion evidence and must not be counted as a pass.

The [twenty-ninth Paseo qualification](evidence/paseo-2026-09-21/live-run-29.md)
also **failed** before implementation completed. The real Codex worker and
Claude coordinator both received the built-in Cortex MCP catalogue, but their
external MCP inventories were empty even though codebase-memory and Playwright
were configured. The production wrapper omitted the host MCP server argument.
The run was stopped and archived immediately; Claude QA, Playwright delivery,
report handoff, rendered checks, and task acceptance remain `NOT RUN` for this
revision. A fresh run after the forwarding repair must verify the actual
external inventory in each role before claiming completion.

The [thirtieth Paseo qualification](evidence/paseo-2026-09-21/live-run-30.md)
also **failed**. The coordinator and Codex worker each received exactly the
configured `codebase-memory-mcp` external server, resolving the run-29
delivery failure for those observed roles. The Codex worker then used native
`mcp__cua_repl.js` to inspect browser state and twice attempted a Chrome
`file://` tab during a CLI-only/no-main-GUI run. The first call failed on an
unavailable visibility capability; the second was blocked by URL policy.
No page-open or rendered result was observed. A separate native patch call
that tried Delete and Add for the same file also failed before a corrected
edit. The independent static checker passed 8/8 after the run stopped, but
there was no worker report or result, independent QA, current pipeline
decision, or task acceptance. Claude QA's Playwright inventory and the
`report_ref`/`report_id` handoff remain untested. A fresh disposable run must
complete these observations without using the main GUI browser.

The [thirty-first Paseo qualification](evidence/paseo-2026-09-21/live-run-31.md)
also **failed**. Scoped external MCP inventories were observed in all three
roles, including Playwright only in the Claude QA worker. The Codex worker
published report `r_56ca4ffdbfbd`, and coordinator and QA each read that
exact report with `read_report(report_id=...)` on their first call. The
project checker passed eight checks, but the implementation publication
claimed nine; this discrepancy was never corrected. QA reached a locally
served rendered page, but then attempted an out-of-project `find /` fallback
for a console log. Its native tool returned a refusal string immediately;
no operator denial, pending Paseo request, or visible user confirmation was
observed for that call. An earlier localhost-server Bash request did appear
as pending in the Paseo CLI and was allowed there; actual chat/UI confirmation
visibility was **NOT OBSERVED** because this was a CLI-only run. The agent was
stopped and archived. No QA publication, current final pipeline edition,
coordinator decision or task acceptance occurred, so the orchestration
remains unqualified despite partial rendered observations and zero observed
Cortex MCP errors.

A focused [Paseo permission probe 32](evidence/paseo-2026-09-21/live-permission-probe-32.md)
is **FAIL** for out-of-project confirmation. In a fresh disposable Git
project, native Claude Bash `pwd` returned the project cwd without a pending
host request; the Cortex engine recorded `coordinator_read_auto_approved`.
An exact read of a test-owned sentinel in a sibling directory returned
`User refused permission to run tool` within 10 milliseconds, while Cortex
recorded `coordinator_private_or_project_check_refused` with category
`outside_project`. The permission-receipt table had zero rows and the
Paseo CLI showed no pending request. The refusal arose locally before a
host decision; no user denial was observed. A follow-up “Cortex role
restriction” was delivered as a native `user` message, and the model spent
another turn declining to treat it as authoritative. No headless or main
chat UI confirmation was observed, so UI visibility remains **NOT RUN**.

The next [Paseo permission probe 33](evidence/paseo-2026-09-21/live-permission-probe-33.md)
observed the repaired routing for these exact commands. In a fresh Git
project, native Claude Bash `pwd` auto-approved, while `cat` of a test-owned
sibling sentinel produced a pending Bash request visible through Paseo's CLI.
The outside read was never approved or executed; the owned agent was stopped
and archived to clear the request. The available CLI and sanitized engine
trace did not expose the engine's permission option kinds or IDs, and no
headless or main chat confirmation card was observed. Those UI and
user-decision cases remain **NOT RUN**; a pending CLI record alone does not
qualify them.

A further [overlapping-turn retry](evidence/zed-2026-09-19/live-direct-project-overlap-failure.md)
launched a real Codex worker in the exact Zed project and passed the project's
unit test, but **failed** task completion: the end-of-coordinator-turn guard
attributed the concurrent worker edit to the coordinator and blocked the task.
The worker-publication revision repair requires a new real-client run; the
earlier blocked task remains a failure.

The [next direct-project task](evidence/zed-2026-09-19/live-task-decision-recovery.md)
proved worker publication, project-root test execution and recovery after
explicit steering, but **failed autonomous completion**: the coordinator
accepted the assignment twice and omitted the overall task decision. The
task reached `accepted` only after a separate user-style clarification.
The decision-scope instruction and tool-feedback repair needs a fresh run.

The [2026-09-20 real ACP baseline](evidence/zed-2026-09-20/live-acp-baseline.md)
observed autonomous task acceptance for a Codex worker, actual worker/tool-card
replay after a test-Zed restart, and a completed Codex-coordinator/Claude-worker
task. A second Codex/Claude task repeated assignment acceptance without a task
decision and was cancelled. The Zed selector displayed Claude for one task
whose recorded coordinator route was Codex. Both accepted tasks left their
bootstrap-only `pipeline.md` stale. These findings require a new run after
their fixes; they do not change the older failed observations.

The [focused regression runs](evidence/zed-2026-09-20/live-acp-regressions.md)
then reproduced the stale-pipeline defect and verified its completion gate in
a fresh direct-project Codex task: Zed showed a final answer after the task
accepted with a coordinator-authored current pipeline edition. The earlier
failed runs remain part of the record. The recovery-attempt fix, selector
alignment, deletion permissions, and all other unrun cases remain unverified.

A later production-Zed prompt was reported to fail at the then-mandatory
`bubblewrap` user-namespace probe; matching AppArmor denials were observed.
See the [sandbox refusal record](evidence/zed-2026-09-19/production-sandbox-denial.md).
That failure is retained as historical evidence. The current ACP path does not
require `bubblewrap` or an AppArmor repair: it launches native engines
directly with the user's OS permissions. A fresh
production observation is still required to qualify the current path.

**Known harness limitation (F5, T011 review):** `scripts/cortex_acp_smoke.py`
cannot detect a stray write to the real stdout that happens *before*
`cortex_runtime.acp.agent.main` redirects `sys.stdout` to `sys.stderr` --
by the time the harness's reader thread starts consuming stdout, any such
write already landed ahead of the first protocol frame, and the harness has
no way to distinguish that from the client itself misbehaving. The
"stdout-clean verification via logs" row below (and, independently, a
manual `python3 -B src/cortex_acp.py < /dev/null | head
-c 200 | xxd` byte-level check) is the authoritative check for this
specific failure mode, not the automated harness.

## Preconditions for T035

For Paseo observations, start the isolated daemon from the disposable project
directory before querying the provider catalog or opening a session. Paseo's
`provider models` command sends ACP `session/new` using the daemon's inherited
cwd. Starting it from the Cortex checkout can select that checkout's `.cortex`
store while the test targets another project, which Cortex rejects as
`project_storage_mismatch`. Restart the isolated daemon from the exact
disposable project root and retain the command/cwd in the observation record.

For Luna-only paid qualification, write
`{"pin": {"*": "codex-acp/gpt-5.6-luna:high"}}` to the disposable project's
`.cortex/routing.json` **before** starting the client session. This
project-local file outranks the user's own overrides and routes every worker
role to `codex-acp/gpt-5.6-luna` at `high`; if Luna is unavailable the
delegation is refused before engine launch. Inspect the durable route audit
(`route_basis: auto:pin`) for each delegated attempt. Do not install this
file into the user's global Cortex config.

- Zed 1.20.1 (or the then-current qualified version — re-verify and record
  the exact version at run time) with Cortex registered via
  `scripts/cortex_acp_client_config.py --apply --i-consent-to-write` or the
  manual snippet in [zed-custom-agent.md](zed-custom-agent.md).
- codeg 0.30.8 (or the then-current version) with Cortex registered per
  [codeg-custom-agent.md](codeg-custom-agent.md) — contingent on resolving
  the UNKNOWN raw-local-command question first.
- At least one real, user-approved, non-fake connection configured through
  Cortex's own onboarding for the "selector rendering" and "connection
  repair" rows; a paid model call still requires separate, explicit user
  authorization per row (see `INVARIANTS.md`: "Install with pinned versions
  and explicit consent; diagnostics is non-paid and full probes are
  opt-in").

## Observation table

| Client | Version | Step | Expected | Observed | Evidence path | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Zed | 1.20.2 | Cortex appears as a separate agent in the Agent panel | "Cortex" listed distinctly from built-in providers and other custom agents; no auto-activation from `.cortex` presence alone | Cortex appeared under External Agents; an existing `.cortex` alone did not open it | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | Start a session with Cortex | Session opens; ACP `initialize`/`session/new` succeed; no stdout protocol pollution (verify via Zed's own agent log, not just this harness) | Zed's ACP view showed valid `initialize` and `session/new` request/response pairs as first frames; a direct EOF launch emitted zero stdout/stderr bytes | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | Selector rendering, no connection configured | No Model/Thought-level selector shown (never an empty/fake one) | Only Language appeared; no Model or Thought level control | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | Selector rendering, one connection configured | Model selector shown with real route id(s)/labels; Thought level shown iff the route reports one | Registered connection remained authorization-required; no selectable route existed | [observations](evidence/zed-2026-09-19/observations.md) | BLOCKED |
| Zed | 1.20.2 | Command fallback | `/cortex-status`, `/cortex-model`, `/cortex-effort`, `/cortex-setup` work as slash commands even if Zed's own config-option UI is used instead | All four dispatched and returned actionable responses without a model; model and effort selection with a real route remain unverified | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | Progress coherence | Streamed `agent_message_chunk` updates render incrementally and coherently; final `stopReason` matches what the UI shows as "done"/"cancelled" | No authorized engine route; no long-running model turn | [observations](evidence/zed-2026-09-19/observations.md) | BLOCKED |
| Zed | 1.20.2 | Reopen a recorded ACP session after restarting the isolated Zed editor | Text plus structured worker/tool cards return in order through `session/load` | A deterministic no-paid runtime emitted two text chunks and worker/tool cards; after editor restart and project reopen, Zed showed the same ordered cards and text. Real worker execution remains unverified | [history replay](evidence/zed-2026-09-19/history-replay.md) | PASS |
| Zed | 1.20.2 | Setup/repair messaging | `/cortex-setup` guidance is legible in the UI and never shows a secret; a broken/unauthorized connection shows a truthful repair message, not a silent failure | Zero-connection and authorization-required messages were legible and exposed no secret | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | Uninstall | Removing the `agent_servers` entry removes Cortex from the panel; no Cortex connection/credential state is affected | Isolated entry removal hid Cortex; registry hash and isolated engine directory were unchanged | [observations](evidence/zed-2026-09-19/observations.md) | PASS |
| Zed | 1.20.2 | End-to-end counter repair | Worker edits the exact project open in Zed; checks and evidence precede a truthful completion | Workers did not launch; only coordinator copy changed; project test still failed while final answer claimed success | [failure record](evidence/zed-2026-09-19/live-counter-failure.md) | FAIL |
| Zed | 1.20.2 | Direct-project counter repair retry | Connected worker edits the exact Zed project, the test passes there, and completion is evidence-bound | First Claude assignment failed `engine_not_connected`; user stopped the run before the Codex retry could complete; project test still failed | [failure record](evidence/zed-2026-09-19/live-direct-project-failure.md) | FAIL |
| Zed | 1.20.2 | Direct worker overlaps coordinator turn | Worker edits the exact Zed project and the task accepts only after current evidence | Worker changed the project and independent unit test passed; runtime blocked completion as a coordinator write | [failure record](evidence/zed-2026-09-19/live-direct-project-overlap-failure.md) | FAIL |
| Zed | 1.20.2 | Direct-project task decision | Worker result and test lead to an overall task decision without extra steering | Worker/test succeeded, but assignment-level accepts did not finish the task; later steering produced an accepted task | [recovery record](evidence/zed-2026-09-19/live-task-decision-recovery.md) | FAIL |
| Zed | 1.20.2 | Fresh direct-project Codex repair | Codex worker edits the exact Zed project, publishes a report, and the coordinator accepts the task after an independent check | Exact project file changed, independent unit test passed, report published, task-level accept recorded without steering | [2026-09-20 baseline](evidence/zed-2026-09-20/live-acp-baseline.md) | PASS |
| Zed | 1.20.2 | Real worker/tool-card replay | After restarting test Zed, text and actual structured worker/tool cards return in order | Same thread reopened with its worker and tool cards visible; 23 structured updates retained | [2026-09-20 baseline](evidence/zed-2026-09-20/live-acp-baseline.md) | PASS |
| Zed | 1.20.2 | Mixed Codex coordinator and Claude worker | Worker edits the exact project, checks pass and task accepts | One task accepted; a second identical-route task repeated assignment accepts and was cancelled | [2026-09-20 baseline](evidence/zed-2026-09-20/live-acp-baseline.md) | FAIL |
| Zed | 1.20.2 | Selector/effective route consistency | The route shown before sending matches the coordinator route actually launched | Zed showed Claude Sonnet but runtime recorded Codex as coordinator for that task | [2026-09-20 baseline](evidence/zed-2026-09-20/live-acp-baseline.md) | FAIL |
| Zed | 1.20.2 | Current pipeline edition | Accepted task has a newest-first edition reflecting the decision and evidence | Both accepted live tasks retained only the initial “not yet worked” edition | [2026-09-20 baseline](evidence/zed-2026-09-20/live-acp-baseline.md) | FAIL |
| Zed | 1.20.2 | Current pipeline edition after completion gate | A fresh task cannot accept with only the bootstrap edition; coordinator publishes its evidence and intended decision before overall acceptance | Direct-project task `t_e68f2a68cc52` accepted after coordinator edition 5, distinct assignment/task accepts, complete candidate check, and a final Zed answer. The exact final restamped payload still needs offline checks | [focused regression](evidence/zed-2026-09-20/live-acp-regressions.md) | PASS |
| Zed | (record exact) | Access selector | An **Access** selector shows Guarded mode for a new session; choosing Auto mode is recorded as the task's access revision | | | NOT RUN |
| Paseo | (record exact) | Access shield | The composer mode control shows Guarded mode (safe shield) for a new chat and Auto mode (open, dangerous shield) after switching; a new chat starts Guarded | | | NOT RUN |
| Zed or Paseo | (record exact) | Auto mode run | In Auto, a worker deletion and an `outside_command` complete without a card, a Claude worker's confirmed mode is `bypassPermissions`, and `.cortex`/`.git/config` stay protected; back in Guarded the next deletion shows a card | | | NOT RUN |
| codeg | (record exact) | Cortex appears as a separate agent | Same as the Zed row, in codeg's own agent list UI | | | NOT RUN |
| codeg | (record exact) | Start a session with Cortex | Session opens; codeg's injected `codeg-mcp` server is present in `session/new` params but never trusted/routed by Cortex | | | NOT RUN |
| codeg | (record exact) | Selector rendering, no connection configured | Same as Zed row | | | NOT RUN |
| codeg | (record exact) | Selector rendering, one connection configured | Same as Zed row | | | NOT RUN |
| codeg | (record exact) | Command fallback | Same as Zed row | | | NOT RUN |
| codeg | (record exact) | Progress coherence | Same as Zed row | | | NOT RUN |
| codeg | (record exact) | Setup/repair messaging | Same as Zed row | | | NOT RUN |
| codeg | (record exact) | stdout-clean verification via logs | codeg's own process/log view shows no non-protocol stdout from the Cortex subprocess | | | NOT RUN |
| codeg | (record exact) | Uninstall | Same as Zed row | | | NOT RUN |

## Recording instructions for T035

For each row: fill in the exact client version, the actual observation,
a path to retained evidence (screenshot, log excerpt with secrets
redacted, or a saved transcript), and a status of `PASS`, `FAIL`, or
`BLOCKED` (never leave a row `NOT RUN` while claiming the row passed).
Cite the paid-probe/recipient approval reference separately if any row
required one (see `INVARIANTS.md`).
