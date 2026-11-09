# CW-U08: Command and lifecycle controls

**Status:** Planned; design gate passed 2026-11-05; implementation waits for its dependencies · **Phase:** 3+ · **Size:** L

## Goal
Command and lifecycle controls.

## Requirements
CW-R22, CW-R23, CW-R25, CW-R28; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Expose supported Cortex commands as controls, including Stop, Continue and close dialog choices. The close dialog offers Close as is and Cancel task, each with its consequence stated. The stop dialog offers Stop and Continue. Continue is offered only when the runtime has a real resume capability ([CW-C08](CW-C08.md)); pause is not advertised as a separate action before then. The control that stops one running command is part of [CW-U13](CW-U13.md). Split before implementation if effort exceeds L.

## Acceptance
- Positive: Stop and Continue are distinct; Close as is and Cancel task are explicit.
- Negative: Never silently discard work or claim unsupported actions succeeded.
- Negative: Continue is not shown as available without a real resume capability, and Stop never reports a pause.

## Dependencies
CW-C08, CW-U02, CW-U13 (stop one running command).

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
