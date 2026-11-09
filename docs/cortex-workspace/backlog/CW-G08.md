# CW-G08: Per-attempt change attribution

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** M

## Goal

Give each worker attempt an authoritative record of the files it changed, so the finished worker view can show what that attempt changed, not only the recorded edit blocks. Choose one method: a Git snapshot at the attempt's start and end, or attribution from write records. State its limits where human edits or other workers touch the same checkout.

## Requirements

- CW-R32: the attribution method and its limits are labelled. Unknown stays unknown.
- CW-R05: changes are shown as evidence, with the exact revision they come from.
- CW-R11: Git reads are explicit and conflicts are not hidden.
- CW-R19: the attribution depends on the task's checkout choice: the current checkout or a dedicated worktree.

See [requirements](../requirements.md).

## Inputs

- The current limit: the recorded edit diffs may omit side effects of commands ([observability](../../features/observability/index.md)). The finished worker view labels them as "From recorded edits".
- [CW-G01](CW-G01.md) (Git read projection), [CW-G06](CW-G06.md) (optional worktree lifecycle) and [CW-I02](CW-I02.md) (human-mutation lease and writer quiescence).
- Concurrency: a human edit in the same checkout during an attempt makes a snapshot diff ambiguous. The design must decide how to handle it.

## Deliverables

- A recorded decision on the attribution method, with its limits when a checkout is shared.
- A read-only, per-attempt list of changed files, with additions and deletions counts and a per-file diff.
- A label on each result: "Attributed by <method>", plus "may include concurrent edits" when the checkout is shared.

## Acceptance

Positive:

- An attempt running in its own worktree shows exactly the files it changed.

Negative:

- When a human edit happens in the same checkout during an attempt, the result is labelled as ambiguous, not exact.
- When attribution is unavailable, the view shows "Changes unavailable", never an empty list.

## Dependencies

- [CW-G01](CW-G01.md), [CW-G06](CW-G06.md) and [CW-I02](CW-I02.md).
- Gate records: CW-D05 (Gate D) must be recorded before implementation.

## Validation

- Fixture tests in a temporary Git repository: a worker-style edit, a concurrent human edit in a shared checkout, and an attempt in a separate worktree. Record the commands and outcomes.
- The result labels for each case are checked against the expected label.

## Documentation impact

- [Observability](../../features/observability/index.md) and [Git](../../features/workspaces-and-integration/index.md) feature pages, only after the attribution ships.

## Size

M: one attribution method, a read-only view and its labels. Split if the method needs more than one.
