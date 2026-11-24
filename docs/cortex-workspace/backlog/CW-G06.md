# CW-G06: Optional worktree lifecycle

**Status:** Implemented in the local desktop service, with no-paid source coverage; see [separate worktrees](../local-task-execution.md#separate-worktrees) · **Phase:** 3+ · **Size:** L

## Goal
Optional worktree lifecycle.

## Requirements
CW-R19, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
The Worktrees settings page ([CW-U07](CW-U07.md)) controls the machine root. Start provisions from committed HEAD and binds execution and checkout operations to that folder. Invalid roots, missing Git and absent committed HEAD refuse without fallback. Cleanup verifies provenance and cleanliness: ordinary close and archive keep the folder, explicit close-and-delete and purge can remove it, and confirmed squash defaults to requesting cleanup when safe. The main checkout and branches are never deleted by cleanup.

## Acceptance
- Positive: Choose current checkout and open branch or a dedicated worktree; bind the session to the chosen directory. A dedicated worktree is created under the configured root and bound to its task.
- Negative: Unsafe concurrent writes and unconfirmed deletion are blocked; do not assume shared-checkout safety. An invalid or unavailable worktree root blocks creation and is never replaced silently.

## Dependencies
CW-F04, CW-I02, CW-D06a.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
