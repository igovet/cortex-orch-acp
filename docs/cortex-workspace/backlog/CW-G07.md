# CW-G07: Owner-confirmed merge and PR

**Status:** Implemented in desktop and service source: owner-confirmed squash or PR, explicit Cortex handoff for a stopped source merge, recovery and fresh-check request ([Integration and conflicts](../local-task-execution.md#integration-and-conflicts)); no paid or installed-desktop qualification for handoff · **Phase:** 3+ · **Size:** L

## Goal
Owner-confirmed merge and PR.

## Requirements
CW-R20, CW-R25, CW-R35; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Ready to merge, target selection and confirmed local squash or PR workflow. **Hand to the app…** sends the saved conflict to Cortex only on owner confirmation. Finish requires fresh checks; inspect unconfirmed outcomes before attempting manual changes.

## Acceptance
- Positive: Default target is the Git default branch, overridable for the work or sprint; the owner confirms final integration. A confirmed stop or failure returns a still-stopped saved merge to the owner. After manual or Cortex Finish, review the normal diff and request fresh independent review and runtime verification before integration.
- Negative: Automatic mode never merges or creates a PR without confirmation; conflicts remain unresolved until the owner hands them to Cortex or resolves them. A Cortex resolution is never merged or pushed without owner confirmation, and no automatic stash or reset runs.

## Dependencies
CW-G06, CW-C07, CW-G03.

## Validation
Use no-paid fixtures with a real Git repository to exercise the bound handoff and path manifest, confirmed stop, unknown inspection without replay, exact Finish and Abort, and fresh-check gating. Keep final target integration and target-side squash rollback separate.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
