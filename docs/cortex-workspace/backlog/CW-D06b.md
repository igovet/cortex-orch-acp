# CW-D06b: Prototype sprint domain and queue tests

**Status:** mock built in the browser prototype; unqualified; Gate D passed 2026-11-05 · **Phase:** 1 · **Size:** L

## Goal
Prototype sprint domain and queue tests.

## Requirements
CW-R18, CW-R20, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Test-first sprint planning, versioned dependency DAG and manual/sequential queue. Split before implementation if effort exceeds L.

## Acceptance
- Positive: A valid plan creates linked tasks and sequential mode respects dependencies.
- Negative: Cycles and failed prerequisites block progression; no automatic merge.

## Dependencies
CW-D06a.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built in `prototypes/cortex-workspace/desktop/src/workspace/domain/sprints.ts`: sprint planning conversation, a validated dependency graph that refuses self references, missing tasks and cycles, versioned plans, idempotent task creation, and manual and automatic queue modes. Automatic mode runs one task at a time in dependency order and pauses on a question, failed check, conflict or owner stop. Tests: `domain/phase4.test.ts` and `domain/domain.test.ts`.
- Owner decision of 2026-11-05: an automatic run moves a checked task to Ready to merge by itself, and the owner confirms there. The mock implements it ([CW-D02](CW-D02.md)).
- Not built: a sprint's own archive record and removing a task from a plan while editing.
