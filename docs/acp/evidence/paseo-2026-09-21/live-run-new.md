# Paseo live qualification — new landing run stopped before QA (FAIL)

Date: 2026-09-21 UTC. The repository-owned Paseo plugin was refreshed with
`scripts/update-paseo-plugin.sh` while Paseo was already running. The script
reported package `1.0.0+sha256.48a31218bfacf92d`, payload verification passed,
and plugin `cortex-acp` reached `running`.

The disposable Git project was
`/var/tmp/cortex-paseo-landing-new-LqYca6/project`, initialized with an
initial commit `2b136c5`. The Paseo workspace was
`wks_923be6f9ef8d3f39`, and the agent was
`6ad4299e-89b7-4b11-bf29-a2460b92dbcc`. The Cortex task recorded in the
project-local store was `t_95a542e1a428`. The coordinator route was Claude
ACP, model `sonnet`, effort `medium`, with route audit
`ara_904243f44d23`. The frontend worker route was Codex ACP,
`gpt-5.6-luna`, effort `medium`, with route audit
`ara_be8b500c9c11`. Both were launched through the user's existing production
accounts. No test account or Zed process was used.

The first CLI launch attempt was rejected before an agent was created because
Paseo was given an unsupported Cortex `default` mode. The same prompt was
then relaunched without that native mode and created the agent successfully.
The coordinator inspected the project and delegated the Codex frontend worker.
The worker read the project files, read two Cortex skills, created a draft,
and issued one edit activity. No worker report, complete assignment,
independent Claude QA worker, Playwright observation, current pipeline
edition, or task-level decision was observed.

The run was stopped at 12:56:09 UTC at a safe boundary after a source review
identified a release defect in the completion guard for structured rendered-UI
requirements. The stop produced the expected durable `assignment_cancelled`
event with reason `urgent stop (/cortex-stop)`. The coordinator and worker
processes were terminated, Paseo reported the agent `idle`, and there were no
pending permissions. A coordinator `transport_eof` progress event was emitted
as part of the stop. The overall qualification is **FAIL** and all unobserved
rows above remain **NOT RUN / unverified**. This run must not be used as
evidence that the current package completes a real landing task.

Retained project-local identifiers and event metadata are in the disposable
`.cortex/cortex.sqlite3`; private report bodies and host logs are not copied
into this repository. The fixture remains outside the Cortex checkout for
post-stop inspection.

## Fixed-package retry — interrupted by Paseo daemon shutdown (FAIL)

Date: 2026-09-21 UTC. The plugin was refreshed again with
`scripts/update-paseo-plugin.sh`; the reported package was
`1.0.0+sha256.ea16720ae6ea0fba`, with payload hash
`4a8aeae1e8d99eb50ef6bdfc6ddd3913f5f9b6c364af4cc5d3132994eee537f0`. Paseo
reported `cortex-acp` as `running` before the new chat was launched.

The fresh disposable Git project was
`/var/tmp/cortex-paseo-landing-fixed-20260921-HKogD8/project`, with baseline
commit `62ed4c3`. Its Paseo workspace was `wks_0758558a3e9980e7`, agent was
`221064f2-1628-4d14-8d0e-06186669bc24`, and the Cortex task was
`t_2758abafb267`. The coordinator route was Claude ACP/Sonnet/medium; the
implementation worker route was Codex ACP/`gpt-5.6-luna`/medium. The worker
attempt was `8cad802b9c5d46f883d98e63c237d07a`, and the worker ACP session was
`es_7bd4de6c7a4e`. Existing production engine accounts were used; no Zed or
test account was involved.

Observed progress stopped after the worker was delegated, created an
implementation draft (`d_44b76ea110b9`), and emitted an edit activity. The
project-local event log contains only `user_message`,
`delegation_requested`, `assignment_ready`, and worker activity chunks. No
`write_report`, `complete_assignment`, QA dispatch, report read, Playwright
check, pipeline decision, or task decision was observed. No Cortex MCP
argument error was recorded. The task store still says `running`, but the
coordinator and worker process records have no live PIDs, so this state is
stale and cannot be accepted.

At 2026-09-21T13:15:45Z the Paseo daemon log records `[paseo] Stopping
plugin` followed by `Provider connection closed`. Paseo then emitted
`ERR_IPC_CHANNEL_CLOSED` while closing the provider process. The Paseo main
log records the corresponding desktop daemon shutdown request with
`reason: 'quit'` and a graceful lifecycle shutdown. This is the direct cause
of the interrupted run; the IPC error is shutdown fallout, not evidence of a
successful or failed Cortex tool call. The run is **FAIL** and all end-to-end
qualification rows remain **NOT RUN / unverified**.

## Third run — worker evidence complete, coordinator transport EOF before task decision (FAIL)

Date: 2026-09-21 UTC. The daemon was started headlessly with the normal
production Paseo home and remained running during the run. The installed
package was `1.0.0+sha256.ea16720ae6ea0fba`; Paseo reported `cortex-acp`
enabled and `running` from the repository plugin path.

The fresh disposable Git project was
`/var/tmp/cortex-paseo-landing-third-4RjvnM/project`, baseline commit
`74c486f`. Paseo workspace was `wks_6b211f8014c15604`, project id was
`prj_14644d0dd1917799`, and agent was
`dd7b40e7-139e-41e5-89fd-3a1e2b0b98bc`. The Cortex task was
`t_d58de35afe5a`. Routes were authorized as follows: coordinator
`co_f8b588942fd6` Claude ACP/Sonnet/medium; implementation worker
`xa_99d2eead9fb8` Codex ACP/`gpt-5.6-luna`/medium; QA worker
`xa_1870d4cd7ea7` Claude ACP/Sonnet/medium.

The implementation worker published report `r_7e84908260af` and completed its
assignment. The QA worker read that exact report, published independent report
`r_1239a01f5fb9`, and completed its assignment. The required checker passed
all eight checks with exit code 0; the actual diff was limited to
`index.html` and `styles.css`; `git diff --check` and the external URL scan
passed. QA also observed HTTP 200, visible title and signup form, responsive
1280px/375px layout, and visible keyboard focus outlines through Playwright.
The first file-URL browser attempt was blocked by the browser policy, after
which QA used the same project over a local HTTP server. A cleanup command
containing `rm -f /tmp/qa_httpserver.log` was not approved and failed; QA
recovered, completed its checks, and the test-owned server was subsequently
stopped. No Cortex MCP argument error was recorded.

The coordinator recorded assignment-level `accept` decisions for both worker
assignments, but Cortex correctly returned `assignment_accept_requires_task`;
the required durable task-level decision had not yet been recorded. QA
integration was a no-op (`applied_paths: []`), then the task store received
the durable `/cortex-stop` marker at `2026-09-21T13:31:35.063799Z`. The owned
coordinator process exited at `13:31:35.581286Z`, and the resulting
`transport_eof` progress event was recorded at `13:31:35.601935Z`. The EOF
was therefore stop fallout, not an independent coordinator engine failure.
The task store recorded `state=stopped` with explanation `Stopped by
/cortex-stop`; no
`coordinator_decision` task outcome or published pipeline edition exists.
The follow-up sent after that point was stored only as a
`post_stop_message`, so it could not change the stopped task. The worker
reports and evidence remain durable, but the end-to-end orchestration is
**FAIL** and must not be accepted. The host ACP session still has
`closed_at=NULL` despite terminated process records, which is an additional
state-reconciliation defect observed in this run. The current package now
closes task-bound ACP host sessions during future durable stops; this
historical `closed_at=NULL` observation remains unchanged.
