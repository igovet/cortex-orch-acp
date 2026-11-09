# CW-M03: Mobile file/Git conflict flow and desktop return; owner Gate M

**Status:** planned · **Phase:** 2 · **Size:** M

## Goal
Mobile file/Git conflict flow and desktop return; owner Gate M.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
A clickable conflict and return-to-desktop scenario and owner decision recorded in [mobile approval](../design/mobile-approval.md). An absent or conditional decision does not pass Gate M.

## Acceptance
- Both routes show file and Git conflicts and return to the same synthetic desktop task; Gate M remains open absent unconditional owner approval.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Dependencies
CW-M02

## Validation
Mobile conflict and return walkthrough: simulate both conflicts and desktop return; inspect Gate M record for selection or requested revisions. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.
