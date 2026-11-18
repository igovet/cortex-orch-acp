# CW-H05: Machine registry and connector

**Status:** Partly implemented: the phone keeps a registry of paired machines and connects through the remote tunnel; the desktop's own machine registry and connector are not implemented and its Add remote machine control stays disabled; not verified on a device · **Phase:** 3+ · **Size:** L

## Goal
Machine registry and connector.

## Requirements
CW-R17, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Build test-first local machine bootstrap, project ownership and remote connector registration. Split before implementation if effort exceeds L.

## Acceptance
- Positive: One local machine can contain several projects; remote machines are explicitly connected.
- Negative: Projects never become machines; disconnected remote data is not presented as current.

## Dependencies
CW-H02.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.
