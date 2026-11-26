# Decision: live sprint graph ownership, start fence and priority

**Status: accepted and built in the local Workspace service and desktop. Verified by focused no-paid tests; not live-qualified.**

## Context

Create tasks turns an approved plan version into drafts. Afterwards the owner may add more tasks to the sprint with **New task**. Those tasks carry no plan key, had no links and used to sit last on the Board and List with no way to say when they should run. The owner needs to see the whole graph that will actually run, change its links, and see the tasks that are working or waiting in the order they will run.

## Decision

1. **Two graphs, two owners.** The approved plan (`plan`, `history`, `materializedVersion`) is historical record that the live-graph actions and the desktop view never write after Create tasks (the desktop shows it read-only). This is not a service-wide guarantee: the pre-existing generic sprint patch can still write `plan`, `history` and `materializedVersion` unless the service is separately hardened. The live graph is the sprint roster (`taskIds`), the dependencies saved on each task (`product.tasks[id].dependencies`) and a separate priority sequence. Only the live graph can be edited. The sprint page defaults to Live sprint; Approved plan vN and past versions are read-only. Plan rows bind to tasks by stable task ID, so appending drafts never drops a plan row's state.
2. **Owner edit boundary, one runtime-reviewed revision route.** `product.sprint` actions `live_graph`, `set_dependency`, `move_priority`, `apply_revision` and `reject_revision` are local desktop actions. Paired devices are refused, the generic sprint patch takes no dependency, revision or priority field, and no ACP wire field exposes them. *Revised by the owner decision of 2026-11-25:* the model never edits links or drafts directly, but when the owner changes a running sprint task (an owner message delivered to its coordinator, an owner rework request or an owner answer) while other sprint tasks are still unstarted drafts, the runtime requires that task's coordinator to re-check them. It reads them with `sprint_graph` `include: "pending_descriptions"` and records a verdict with the coordinator-only tool `sprint_revision_submit`: `unchanged`, or `revise` with changes of unstarted drafts only (update, add, propose cancellation). The task cannot finish before the verdict (`sprint_impact_pending`). The service validates a revision under the machine lock with the same ID-only validator and the same draft and prerequisite rules as an owner edit, against the expected draft versions and `graphRevision`. A revision gets a mandatory independent review; the runtime applies an approved one in an Automatic sprint in one project-store transaction with the product record, and the owner applies or rejects it in a Hybrid or Manual sprint. Proposed cancellations are only shown to the owner. Drafts a pending revision changes do not start (`sprint_revision_pending`, no hold); started, finished, archived and purged tasks, the approved plan, its history and `materializedVersion` are never written, Pause is never lifted and nothing is deleted.
3. **One validated, revision-checked edit.** An edit names the `graphRevision` it was made from. Under the machine lock the service checks the revision and the bindings and validates the whole candidate graph with the ID-only validator shared with materialization (self, missing, cross-sprint, duplicate, cycle). It replaces only the dependent's link set in one atomic private product-file replacement. Only a saved, not-started draft without an execution binding, in an open, created, non-deleting sprint, may change incoming links; new links to cancelled, unconfirmed, purged or archived-unsettled prerequisites are refused, while a retained bad link can be removed. The same desired state is a no-op. An unconfirmed save is not retried: the client reads again.
4. **Start fence.** A queued start (Hybrid and Automatic, project folder and worktree) rechecks the saved sprint, links, priority and relevant task facts under the machine lock, before the existing claim. A pick made from an older graph is dropped and reconsidered, with no hold recorded. Manual Start keeps its unchanged admission path. An edit therefore cannot be overtaken by a stale scheduler choice, and an edit never lifts the owner's Pause or rematerializes.
5. **Priority only breaks ties among ready tasks.** Dependencies decide readiness. The priority sequence is stored apart from `taskIds`, so roster and node numbers stay stable. It defaults to roster order, a new member is last, removed members are dropped, and a move swaps with the nearest editable draft. Priority never overrides a dependency, a hold or a Pause.
6. **One order projection.** The service computes the order (`working` by durable start order; `notStarted` by rank with a reason when unranked) from the same readiness and priority rule the scheduler uses. Board, List, the toolbar and the queue bar render it and never recompute it. **Next** is shown only when the service names it, at most one task, for a running queue with a live runtime. Manual shows suggested order and no Next.

## Alternatives rejected

- **Edit the approved plan and rematerialize.** It would rewrite history, task identities and started work.
- **Let the model edit links freely.** Orchestration stays runtime-owned. Since the owner decision of 2026-11-25 a coordinator may only *propose* a revision of unstarted drafts after an owner change of its own task; the runtime validates, reviews and applies it (or the owner does), and a model proposal never cancels a task.
- **Re-plan the sprint from scratch after an owner change.** It would rewrite started work and the approved plan; only unstarted drafts are revised.
- **Client-side ordering.** Two screens could disagree with what the scheduler starts next.
- **Only dependencies, no priority.** Ten independent late drafts could be ordered only by inventing links between them.

## Consequences

- Older services, offline caches and partly read sprints stay read-only, with the reason shown.
- A concurrent edit is refused with `stale_graph_revision`; the owner sees the new graph and repeats.
- The live graph may diverge from the approved plan; the page says how.

## References

[Live sprint graph and execution order](local-task-execution.md#live-sprint-graph-and-execution-order), [protocol](protocol.md), [domain model](domain-model.md), [screens](SCREENS.md#live-sprint-graph-links-and-order-desktop), [SECURITY.md](../../SECURITY.md).
