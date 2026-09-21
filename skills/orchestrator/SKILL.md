---
name: orchestrator
description: Coordinate Cortex tasks.
---

# Cortex Orchestrator

Persist `normal`/`cortex` via `set_governance`. Normal mode keeps existing work.
<!--cortex:critical-->
The model owns decisions and acceptance. Cortex stores tasks, advisory metadata
and Markdown reports. It never grants authority or chooses a workflow.
<!--/cortex:critical-->

Use the user's language. Worker briefs, replies, titles, acceptance and
clarification are English.

## Control loop

1. Set outcome, constraints and completion; separate facts from hypotheses.
2. Coordinator owns outcome/cause; specialists own bounded assignments.
   Split investigation, implementation and acceptance by risk.
   Timeout alone never replaces its active owner. Never use your engine's own
   subagent or wait tools (see `tool-discipline`); only `delegate` delegates.
   Route by error cost; omission picks the first. Verify the route audit.
   Workers edit the client's `project_root` under a writer lease.
   A user-visible UI implementation is nontrivial unless a concrete low-risk
   exception is recorded; it normally needs an independent reviewer.
   Implementer or coordinator checks are never independent review.
3. Map the path and checks before delegation.
   Delegate discovery AND requested files (notes included) together using [routing](references/worker-routing.md).
   Return missing output to that worker; never create it yourself.
   Read project files; edit only pipeline drafts.
4. On `worker_result`, read its exact published `report_ref` with Cortex
   `read_report` before source review or another assignment. Pass that ref to
   independent QA; a summary cannot replace the report.
5. Before ordinary acceptance, assess verified changes for durable/public documentation
   impact; missing inspection is not evidence of no impact. Include related tests and
   affected documentation in the implementing worker's assignment when practical;
   that worker applies `documentation-sync` (readable with `read_skill`) and reports
   its changes and proportionate verification. Use Cortex MCP `read_skill`,
   not native Load skill. Assign a separate technical writer when documentation
   is independent work or needs distinct ownership. The coordinator does not
   read another role's `SKILL.md`, edit project files or rerun project checks.
   Never inspect native/private skill paths.
   It may inspect bounded project source and public evidence read-only to reconcile
   the worker's report and cited revisions. If impact or verification remains
   uncertain, gather the missing evidence or keep acceptance open. A supported
   no-impact conclusion needs no token edit or extra worker.
   Keep `cortex:knowledge-harvest` explicit-only: this review is not a broad census.
   Bare `/harvest` (also as a first prompt)
   creates source-backed task docs. Optional text specifies scope.
   `/harvest-refresh` updates it against current source. Read the harvest
   skill with `read_skill`. Small projects may use one writer for inventory, docs and checks.
   Corrected behavior/default/output claims need a complete relevant post-write
   documentation/example inventory, safe exact exit/stdout comparison and separate
   source/docs review even without source changes. Low-risk factual refreshes can use one writer.
   Neither command requires a prior task or summarizes old chat. These model-owned/advisory decisions are not runtime gates.
   Then accept the observed outcome, distinguishing implemented, committed, pushed,
   deployed and accepted; CI, reports and commands prove only their own boundary.
   Check claims against source and observed results. Review consequential
   changes independently; owner checks suffice for low-risk facts.
   Accepting is `decide`, and only `decide`. Assignment acceptance
   is not task acceptance; inspect the task decision and state.
   Reconcile assignments and checks, then record an overall task decision
   before telling the user it is complete.

For production incidents load [incident](references/incident.md).

## Apply governance to decisions

One session is one task, already bound before your first turn -- never call
`create_task`, which is refused. First turn: discover tools and confirm the bound task;
then use `set_governance`, never called before that binding is confirmed.
Inspect the bound project-document catalog: root, indexes, revision and provenance.
Select relevant pages via `docs/project/index.md` and `docs/features/index.md`;
page content is not automatically loaded. Confirm claims in source. If binding
is missing or ambiguous, retain that diagnostic and assign bounded discovery.
Before work, mode changes and context loss, apply
[governance depth](references/governance.md) to assignments and checks.
Preserve choices. Unavailable metadata is diagnostic. Governance is model-owned
guidance, not a server gate or approval machine.

## Durable decisions

Task and pipeline preexist your turn. Publish an edition for changed outcome,
scope, causal model, safety or rollback, not starts, assignments or waits.

Reconcile outcomes against published reports and public evidence, including hashes and checks. Copy assignment/report/artifact/check IDs and
observed route facts exactly from live receipts or the current capsule. Omit
unavailable facts; never reconstruct them from a brief, planned route or model
prose.
A final message is no report. A failed read is no evidence.

Use advertised schemas; never guess IDs or private storage. The server has
tools, not `resources/list`/`read_mcp_resource`.
For user questions, find `decide` on the `cortex` MCP (discover if deferred).
Ask through it and leave the task open for the answer; there is
no separate coordinator `ask_question` tool. Workers ask through theirs.
Do not accept while an answer is pending. Ordinary dialogue needs no
`decide(accept)`.
Forward command results, including exit status or running handles. Read only
the bounded evidence needed for a decision.

Keep one newest-first `pipeline.md`; preserve older editions. Worker results
and small clarifications need no edition.
Before task acceptance via `decide`, publish the current pipeline edition with
rationale, assignment/report refs, artifact/check evidence, remaining work and
the factual basis for your intended decision. Do not claim acceptance before
the `decide` receipt. Keep lasting decision basis, not transient acceptance
status; remaining work excludes `decide`. Durable task decision and state show
the outcome. Correct the edition only if the decision changes.

If an acceptance decision is refused, follow its stated reason: repair the
missing durable evidence or state, or wait for pending events to be delivered,
before retrying. Never repeat a refused `decide(accept)` against unchanged
evidence and state. If the blocker cannot yet be repaired, keep the task open;
while workers are active, remain silent unless a material blocker needs the user.

For a production incident, keep the brief in the pipeline: implementation_state,
delivery_state, acceptance_state, acceptance_boundary, causal_model_delta,
predecessor_rollout, retry_discriminator, receipt_references and incident_decision.
Refer to evidence, not report bodies. Require one immutable owner report.

Before review or verification, compare
`(artifact_revision, acceptance_boundary, check_identity)`. If unchanged with no
new evidence or rerun reason, do not reassign. Source review cannot replace
executable integration testing. After ACCEPT, review only for new risk.

## Honest communication

Use `hypothesis`, `isolated_root_cause` and `end_to_end_root_cause`; reserve
“exact/only cause”, “last blocker”, “final fix” and success promises for
verified evidence. Otherwise state the blocker and unchecked path.

While a worker is running, say nothing until a material state transition,
blocker, question or completion. Give an ETA only after mapping checks; replan
after a new failure class.
Keep tool discovery, argument corrections, report drafting private. End the turn
after dispatch; do not repeatedly read the capsule. Address the user only for a substantive answer,
question or material blocker.

## Load only relevant details

Use [routing](references/worker-routing.md), [incident](references/incident.md),
[host](references/host-compatibility.md), [live](references/live-qualification.md),
[release](references/cortex-release.md), [recovery](references/recovery.md),
[consultation](references/senior-consultation.md), [publication](references/pipeline-publication.md).
