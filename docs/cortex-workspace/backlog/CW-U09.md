# CW-U09: Machine switcher and aggregate views

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** M

## Goal
Machine switcher and aggregate views.

## Requirements
CW-R17, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Provide one-machine-at-a-time switching and aggregated Inbox and Archive. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Projects follow the selected machine; aggregate views identify their source.
- Negative: Unavailable machines and stale data are not silently omitted or marked current.

## Dependencies
CW-H05, CW-U01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
