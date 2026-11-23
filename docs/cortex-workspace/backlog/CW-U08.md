# CW-U08: Command and lifecycle controls

**Status:** Implemented: commands as UI controls, command stop and lifecycle controls ([maintenance and controls](../maintenance-and-controls.md)); Stop, Resume and Continue show one consistent status and the single applicable action, or the exact reason none applies; no-paid checks only; rendered installed-desktop qualification was not run · **Phase:** 3+ · **Size:** L

## Goal
Command and lifecycle controls.

## Requirements
CW-R22, CW-R23, CW-R25, CW-R28; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Expose supported Cortex commands as controls, including Stop, Continue and close dialog choices. The close dialog offers Close as is and Cancel task, each with its consequence stated. Owner Stop pauses the task and Resume (a versioned control) continues it ([CW-C08](CW-C08.md)). Continue answers an interrupted turn or recovery hold that waits for the owner. One rule decides the control for the board card, task menu, command palette, title band, status strip and composer: Resume for a pause, Continue for a held interruption, no button while the service recovers automatically, and the reason instead of a button while an owner process exit is unconfirmed. Cancelled, failed, closed and unknown tasks never offer Resume or Continue; a new message may reopen eligible finished work with a fresh plan. The control that stops one running command is part of [CW-U13](CW-U13.md). Split before implementation if effort exceeds L.

## Acceptance
- Positive: Stop and Continue are distinct; Close as is and Cancel task are explicit.
- Negative: Never silently discard work or claim unsupported actions succeeded.
- Negative: Resume and Continue are not offered on a cancelled, failed, closed or unknown task, or while exit evidence is missing, and the reason is shown instead.

## Dependencies
CW-C08, CW-U02, CW-U13 (stop one running command).

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
