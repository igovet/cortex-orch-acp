# CW-U06: Archive and purge UI

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** M

## Goal
Archive and purge UI.

## Requirements
CW-R21, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Provide archive, retention display, restore and guarded purge controls. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Show seven-day archive and per-machine 30-day default retention; confirm deletion of unmerged work.
- Negative: Refusal prevents archive or purge; no silent destruction or fabricated cleanup receipt.

## Dependencies
CW-C10, CW-U01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
