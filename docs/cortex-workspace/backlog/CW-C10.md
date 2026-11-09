# CW-C10: Archive retention and guarded purge contract

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Archive retention and guarded purge contract.

## Requirements
CW-R21, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Specify test-first archive markers, per-machine retention, targeted purge and audit; reuse supported locking where verified. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Done auto-archives after seven days and retention defaults to 30 days per machine.
- Negative: At archive or purge unmerged work prompts; deletion requires confirmation; refusal prevents archive or purge.

## Dependencies
CW-C06, CW-D06c.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
