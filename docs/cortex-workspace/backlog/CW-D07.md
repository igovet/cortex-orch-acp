# CW-D07: Prototype port and contract test harness

**Status:** partly built in the browser prototype (port and mock adapter); golden mappings not built; unqualified · **Phase:** 1 · **Size:** M

## Goal
Prototype port and contract test harness.

## Requirements
CW-R15, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Define WorkspacePort, mock adapter, shared golden mappings and acceptance-to-test traceability. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Run shared positive and negative contract cases against mock and mappings.
- Negative: Unknown facts remain unknown; mock results are never real runtime evidence.

## Dependencies
CW-D06a, CW-D06b, CW-D06c.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built: `WorkspacePort` in `prototypes/cortex-workspace/desktop/src/workspace/domain/types.ts`; a memory-only mock adapter with detached snapshots and atomic commands in `src/workspace/adapter/mock.ts`; and fixture and port tests in `src/workspace/adapter/fixtures.test.ts`.
- Not built, per the build records: the shared golden mappings against [protocol](../protocol.md), and the acceptance-to-test traceability script. No runtime adapter exists, and mock results are not runtime evidence.
