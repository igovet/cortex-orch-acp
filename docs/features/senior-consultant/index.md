# Senior consultant

A coordinator or worker may propose independent senior advice for one unresolved
question that would change a concrete decision when available evidence cannot
settle it. The escalation identifies the decision or alternatives and explains
the gap in the evidence. Complexity, node count and Deep planning do not
trigger consultation. Separately, the runtime's automatic recovery ladder (strategies
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

The runtime builds the consultant's packet. It starts with the latest completed
advice it can verify, then adds reports named in the escalation and reports from
the revision being replaced. Each available report includes its SHA-256 and
artifact version. An unknown report ID is marked unavailable. The packet appears
as `dependency_reports` in the consultant's brief.
The packet is bounded: artifact lists shrink first, then trailing reports are counted as
omitted. The consultant reads anything beyond the packet through `read_report`.

The consultant works only from its consultation packet, the task's current
pipeline and retained reports. It communicates in English; its turn wrapper
carries no question-language exception because it has no question tool. On
replanning, the runtime surfaces the exact prior advice reference, even when it
comes from an older revision. Coordinators and workers read it by ID
and reuse applicable advice. A further consultation requires explaining how
the advice was used and identifying a new fact, changed assumption or different
material question. The runtime can reject repeated requests with unchanged
inputs. Assessing the substance of a new question remains the coordinator or
worker’s responsibility.
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
verification and runtime completion remain mandatory. The five worker profiles
remain in canonical skill sources; the redundant TOML profile files were removed.
