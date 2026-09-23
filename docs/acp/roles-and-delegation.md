# Roles and runtime delegation

The coordinator proposes a structured plan and submits synthesis. The runtime
validates the plan, reserves ready nodes, selects the configured role route,
provisions each engine session, delivers a bounded assignment and schedules review.
There is no public model-controlled spawn, model/effort selection or acceptance API.

All 23 existing worker profiles remain generated from their role specializations
and a shared report protocol. Instructions describe intellectual work and evidence;
they are not an authorization mechanism. Workers publish immutable Markdown reports
and submit structured results through their authenticated channel. They receive
explicit predecessor report references, not an automatic copy of the parent chat.
The build-verification profile is retained among these 23 but is runtime-reserved:
it is not plannable and the current runtime does not dispatch it, since command
verification is executed by the runtime itself.

## Instruction delivery

`NAMED_SKILLS` names the shared skills delivered to each role kind; it tells a model which situational guidance it may request through `read_skill`, without pasting every skill into its prompt. The advertised `read_skill` enum is role-scoped. Coordinator channels include the coordinator-only shared skills identified by `COORDINATOR_ONLY_SHARED_ROLE_IDS` (`cortex-control` and `coordinator-communication`); worker and consultant channels do not.

Reference ids are scoped separately: workers can read the worker references (`report-publication`, `code-and-evidence`, and `interactive-resources`), coordinators can read their coordinator references, and both can read the example catalogue (`index` and the `*-example` ids). The runtime checks the channel role and the enum; prompt text only explains when an available skill may be useful and grants no authority.

An independent reviewer is a fresh runtime assignment. Its verdict controls the
review/fix loop, while actual command verification belongs to the runtime. Route
policy remains configured by the user and resolved by the runtime for each role.

Requested scopes are validated before reservation and enforced by the runtime
gateway and by the engine jail around each worker's engine process.
Scope expansion requires a structured proposal and replanning; it never changes
an active worker’s authority in place. See [containment](permissions-and-containment.md)
and [qualification status](runtime-rewrite-status.md). Prompts grant no authority.
