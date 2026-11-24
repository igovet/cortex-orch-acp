# CW-C11: Description-first generated title contract

**Status:** Implemented: an editable task title and Commit, squash and Push drafts use a short ACP engine session on the cheapest allowed model of the least-used connection ([maintenance and controls](../maintenance-and-controls.md)); no paid generation verified · **Phase:** 3+ · **Size:** M

## Goal
Description-first generated title contract.

## Requirements
CW-R24, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Specify test-first description-only creation and LLM-generated editable title, with failure and retry states. Split before implementation if effort exceeds L.

## Acceptance
- Positive: A description creates a task without requiring a title; generated title can be edited.
- Negative: Generation failure is visible and never silently overwrites a user edit.

## Dependencies
CW-C01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
