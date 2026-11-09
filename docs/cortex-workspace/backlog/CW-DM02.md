# CW-DM02: Bounded contexts and runtime translation

**Status:** partly built; bounded-context document and ADR 0012 and 0013 absent · **Phase:** 1 · **Size:** M

## Goal
Bounded contexts and runtime translation.

## Requirements
CW-R15, CW-R25; see [requirements](../requirements.md).

## Inputs
[Domain model](../domain-model.md), [testing and acceptance](../testing-and-acceptance.md), [desktop rework](../design/desktop-rework-proposal.ru.md) and dependencies below. Owner decisions supersede conflicting historical proposal text.

## Deliverables
Map bounded contexts, ownership and anti-corruption translation to existing public runtime contracts; record unresolved persistence decisions without asserting an ADR decision. Split before implementation if effort exceeds L.

## Acceptance
- Positive: Mappings identify authoritative runtime facts and unknown fallback.
- Negative: Do not redesign runtime state transitions or invent an approved storage decision.

## Dependencies
CW-DM01.

## Validation
Write positive and negative domain or contract tests first where applicable; run focused tests and record command, environment, outcome and unrun cases.

## Documentation impact
Update the relevant planned contracts and [roadmap](../roadmap.md) as behavior and qualification change; do not describe plans as shipped.

## Status record

- Built in code: the runtime-to-column translation in `prototypes/cortex-workspace/desktop/src/workspace/domain/projections.ts` is conservative, and unknown runtime facts fall back to unknown. The bounded contexts are described in [domain model](../domain-model.md).
- Not present on 2026-11-05: a bounded-contexts document with an anti-corruption map, and ADR 0012 and ADR 0013 (the ADR directory holds 0001 to 0011). No storage decision is asserted here.
