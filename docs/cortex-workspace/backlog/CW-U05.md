# CW-U05: Sprint planning and execution UI

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Sprint planning and execution UI.

## Requirements
CW-R18, CW-R20, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Provide planning orchestration, questions, linked tasks, dependency display and manual/automatic run controls. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Owner can review a plan and run eligible tasks sequentially or manually.
- Positive: in automatic mode a task whose checks pass moves to Ready to merge by itself, with no separate Accept step; the owner confirms the merge or PR there (owner decision of 2026-11-05, [CW-R20](../requirements.md)).
- Negative: Failure pauses automatic execution; every task still reaches owner-confirmed Ready to merge.

## Dependencies
CW-C09, CW-U01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
