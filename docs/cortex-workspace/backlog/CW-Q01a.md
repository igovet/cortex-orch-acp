# CW-Q01a: Deterministic fake-ACP desktop scenario

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q01](CW-Q01.md) for the Ubuntu desktop. The phone leg is in [CW-Q01b](CW-Q01b.md).

## Goal
Provide a repeatable desktop-only end-to-end scenario. A deterministic fake ACP agent runs under the machine service. The desktop creates a task, answers a question and a permission request, reads the result and accepts it with its evidence.

## Requirements
CW-R04 (stale decision versions are rejected), CW-R12 (Ubuntu qualification) and CW-R14 (measured responsiveness and accessibility, not claims). See [requirements](../requirements.md).

## Inputs
- [Testing and acceptance](../testing-and-acceptance.md): the desktop fake-ACP scenario is a named acceptance area.
- [Consistency and recovery](../consistency-and-recovery.md) and [protocol](../protocol.md).
- CW-U01 (board, list and task creation), CW-U03 (decision queue), CW-U04 (versioned result screen) and CW-H04 (crash and reconciliation).
- CW-D01 shared fixtures.

## Deliverables
- A fake ACP agent script with a fixed transcript: a fixed message order, one permission request, one question and one result with a digest. It has no network access.
- A scenario runner. It starts the machine service in an isolated data directory, registers a fixture project, and runs: create task → question on desktop → answer → permission → result → accept on desktop.
- A step record with versions and event IDs for each step.
- Teardown that removes the isolated data directory.

## Acceptance
**Positive**
- The full desktop flow completes, and the final state matches the expected snapshot in the transcript.
- Two runs produce the same event sequence, ignoring timestamps. Determinism is checked by comparison.

**Negative**
- A decision answer that carries a stale decision version, submitted by a second scripted client, is rejected. The scenario records the rejection, and the desktop state is not changed by it.
- A missing transcript step fails the scenario. No success is inferred from absent evidence.
- Changing the result digest after review invalidates acceptance. The scenario includes this step.
- Reject stale, unauthorized, missing or conflicting input: the runner rejects a client that is not authorized for the registered project.

## Dependencies
CW-U01, CW-U03, CW-U04, CW-H04. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Rust integration test driving the machine service with the fake agent.
- Rendered Ubuntu run with screenshots of each step.
- No check has been run for this planned item.

## Documentation impact
- [Testing and acceptance](../testing-and-acceptance.md): desktop scenario reference and step list.
- [Roadmap](../roadmap.md): no status change until qualification.
