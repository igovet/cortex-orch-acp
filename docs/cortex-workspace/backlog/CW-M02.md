# CW-M02: Mobile decisions/result/reconnect states, keyboard and large text

**Status:** planned · **Phase:** 2 · **Size:** L

## Goal
Mobile decisions/result/reconnect states, keyboard and large text.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
Extend both routes from CW-M01 with the states and interaction checks in [mobile concepts](../design/mobile-concepts.md), using only shared synthetic fixtures.

## Acceptance
- Both routes distinguish questions, permissions and acceptance, show stale result and reconnect states, and retain usable controls at large text.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Dependencies
CW-M01

## Validation
Decision/reconnect walkthrough: open each state in both routes with large text and keyboard; verify stale and offline actions do not appear committed. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.
