# Senior consultant

A senior consultant is added to a plan by one of two runtime routes. A worker or
coordinator calls `escalation_request`, which may carry an optional `question` and up
to 16 `report_ids`. Alternatively, the runtime's automatic recovery ladder (strategies
`repair`, `escalate_route`, `consult`, `reassign`) reaches its `consult` rung for a task
that keeps failing without progress: it records an automatic consultation request and
the next plan receives the consultant node and packet through the same builder as an
explicit escalation. A plan that names the consultant directly is
refused with `consultant_requires_escalation`. The runtime validates the request, fences
active work and creates the consultant node in the new plan. Automatic routing
selects its model only from models whose `preferred_for` lists `design`; with none
usable, no consultation starts and the reason is visible. There is no availability
substitute for the consultant, and a `*` pin does not bypass the design list. The
automatic replan limit also bounds escalation.

The runtime builds the consultant's packet. It lists the reports the escalation named,
then every report the escalated revision recorded as a result, and gives each report its
current SHA-256 and recorded artifact versions. An id that names no report of the task is
marked unavailable. The packet appears as `dependency_reports` in the consultant's brief.
The packet is bounded: artifact lists shrink first, then trailing reports are counted as
omitted. The consultant reads anything beyond the packet through `read_report`.

The consultant works only from its consultation packet, the task's current
pipeline and retained reports. It communicates in English; its turn wrapper
carries no question-language exception because it has no question tool. Whether
to request another consultation is the escalator's decision, recorded in the
coordinator's `senior-consultation` reference.
Its Cortex tool offer includes report/source-packet reads, `read_skill`, owned
draft/publication and result submission; it is not offered `ask_question` or
`access_request`, `replan_request`, `scope_request` or `escalation_request`
(dispatch refuses all five). It cannot stop work, consume the replan budget or
request another consultation. It is not offered `project_read`, `project_git`,
`command_run` or external MCP servers.
Those Cortex project-source routes are also refused by the gateway with
`gateway_read_denied`. Claude's consultant session disallows native file/search/edit
tools. **Residual limitation:** Codex may still perform auto-approved native reads
inside its read-only engine jail, so packet/report-only is not a complete native
source-access enforcement boundary.

It cannot edit project files through Cortex tools, choose routes or accept the
task. Advice enters the normal report protocol; independent review, command
verification and runtime completion remain mandatory. All 23 role instructions
remain in canonical skill sources; the redundant TOML profile files were removed.
