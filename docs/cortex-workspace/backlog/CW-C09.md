# CW-C09: Sprint planning and scheduler contract

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Sprint planning and scheduler contract.

## Requirements
CW-R18, CW-R20, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Specify and implement test-first plan versions, questions, dependency DAG and manual or sequential automatic scheduling. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Valid planned tasks are created idempotently and launched respecting dependencies.
- Negative: Cycles, failed prerequisites and pending questions block progress; no unconfirmed merge.

## Dependencies
CW-C01, CW-C06, CW-D06b.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
