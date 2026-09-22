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

An independent reviewer is a fresh runtime assignment. Its verdict controls the
review/fix loop, while actual command verification belongs to the runtime. Route
policy remains configured by the user and resolved by the runtime for each role.

Requested scopes are validated before reservation and enforced by the runtime
gateway and by the engine jail around each worker's engine process.
Scope expansion requires a structured proposal and replanning; it never changes
an active worker’s authority in place. See [containment](permissions-and-containment.md)
and [qualification status](runtime-rewrite-status.md). Prompts grant no authority.
