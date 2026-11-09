# CW-M01: Two mobile navigation concepts using tokens selected at Gate D

**Status:** planned · **Phase:** 2 · **Size:** L

## Goal
Two mobile navigation concepts using tokens selected at Gate D.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
Two clickable mock-driven React Native navigation concepts following Gate D, using the visual direction selected at Gate D and shared fixtures; cover Now, task navigation, decision and offline states.

## Acceptance
- Both Now-first and Task-first routes reach task discussion and decisions from the same fixtures; offline actions do not appear committed.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Dependencies
CW-D05

## Validation
Mobile navigation walkthrough: exercise both routes at 360×800 and 412×915 logical viewports, including empty and disconnected fixtures. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.
