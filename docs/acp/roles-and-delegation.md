# Roles and runtime delegation

The coordinator proposes a structured plan and submits synthesis. The runtime
validates the plan, reserves ready nodes, selects the configured role route,
provisions each engine session, delivers a bounded assignment and schedules review.
There is no public model-controlled spawn, effort selection or acceptance API. A coordinator plan requires a constrained `model` recommendation and may include `model_reason`; `list_route_options` provides a read-only live menu. Runtime policy makes the final route and effort decision. See [automatic routing](routing-and-fallback.md#automatic-worker-routing-tiers-model-hints-and-ranking).

All 23 existing worker profiles remain generated from their role specializations
and a shared report protocol. Instructions describe intellectual work and evidence;
they are not an authorization mechanism. Workers publish immutable Markdown reports
and submit structured results through their authenticated channel. They receive
explicit predecessor report references, not an automatic copy of the parent chat.
The build-verification profile is retained among these 23 but is runtime-reserved:
it is not plannable and the current runtime does not dispatch it, since command
verification is executed by the runtime itself.

## Instruction delivery

Read-only roles (`orchestration.contracts.READ_ONLY`: architect, ux-designer, code-reviewer, senior-consultant, explorer, planner, security-auditor, accessibility-auditor) never change the project, whatever scope a plan requests. `plan_submit` therefore refuses a read-only task whose scope names a path that does not exist yet (`read_only_scope_not_writable`): such a path can only be an output the worker could never write. On 2026-09-28 a plan gave seven Opus ux-designer tasks an output file each; every worker did the work and then failed to save it against the read-only project. The task returns content in its report and a dependent writing task creates the file.

`INLINED_SKILLS` are delivered in full with the role instructions because their case arises in almost every session: `tool-discipline`, `content-safety` and `coordinator-communication` for the coordinator; `tool-discipline`, `content-safety`, `report-publication` and `code-and-evidence` for workers. The text is exactly what `read_skill` returns for each id. Naming them was not enough: on 2026-09-28 Claude sessions given the same named list never called `read_skill`, while Codex sessions did. `NAMED_SKILLS` names the remaining situational skills with their triggers; a model requests those through `read_skill`, so they are not pasted into every prompt. The advertised `read_skill` enum is role-scoped. Coordinator channels include the coordinator-only shared skills identified by `COORDINATOR_ONLY_SHARED_ROLE_IDS` (`cortex-control` and `coordinator-communication`); worker and consultant channels do not.

Reference ids are scoped separately: workers can read the worker references (`report-publication`, `code-and-evidence`, and `interactive-resources`), coordinators can read their coordinator references, and both can read the example catalogue (`index` and the `*-example` ids). The runtime checks the channel role and the enum; prompt text only explains when an available skill may be useful and grants no authority.

An independent reviewer is a fresh runtime assignment. Its verdict controls the
review/fix loop, while actual command verification belongs to the runtime. Route policy is configured by the user and resolved by the runtime for each role; per-node model hints are validated recommendations, not authority.

Requested scopes are validated before reservation and enforced by the runtime
gateway and by the engine jail around each worker's engine process.
Scope expansion requires a structured proposal and replanning; it never changes
an active worker’s authority in place. See [containment](permissions-and-containment.md)
and [qualification status](runtime-rewrite-status.md). Prompts grant no authority.
