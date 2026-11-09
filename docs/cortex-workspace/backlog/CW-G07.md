# CW-G07: Owner-confirmed merge and PR

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Owner-confirmed merge and PR.

## Requirements
CW-R20, CW-R25, CW-R35; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Implement test-first Ready to merge, target selection and confirmed local squash or PR workflow. Conflict handoff: "Let Cortex resolve" sends a conflict to the task; its result returns as a normal diff through Ready to merge for owner review, and "Abort merge" is always available. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Default target is the Git default branch, overridable for task or sprint; owner confirms either action. A conflict handed to Cortex returns as a normal diff in Ready to merge, awaiting owner confirmation.
- Negative: Automatic mode never merges or creates a PR without confirmation; conflicts remain unresolved until the owner hands them to Cortex or resolves them. A Cortex resolution is never merged or pushed without owner confirmation, and no automatic stash or reset runs.

## Dependencies
CW-G06, CW-C07, CW-G03.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases. Conflict handoff tests use a real Git repository in a temporary directory: the conflicted state is kept, handoff performs no merge, and abort restores the pre-merge state without stash or reset.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
