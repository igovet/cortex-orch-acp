# CW-Q02a: Desktop fault and race matrix

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-Q02](CW-Q02.md) for the Ubuntu desktop. The phone and device rows are in [CW-Q02b](CW-Q02b.md).

## Goal
Run a matrix of injected faults and races against the [CW-Q01a](CW-Q01a.md) desktop scenario. Assert the documented invariants: no duplicate effects, no acceptance of stale digests, explicit unknown states, no prompt replay after a crash, and no silent overwrite.

## Requirements
CW-R06 (the machine service survives window closure and reconciles crashes without replaying prompts), CW-R12 (Ubuntu qualification) and CW-R14 (measured evidence). See [requirements](../requirements.md).

## Inputs
- [Consistency and recovery](../consistency-and-recovery.md): device races and crash points need executable tests, not UI-only assurances.
- [Protocol](../protocol.md): idempotent requests and typed rejections.
- CW-Q01a (desktop scenario), CW-I03 (hash-checked save, create and rename, which carries the drift check), CW-G04 (ambiguous-outcome reconciliation), CW-G05a (desktop Git panel) and CW-H04 (crash reconciliation).

## Deliverables
- A matrix file. Each row names the fault, its injection point, the expected invariant and the observed result.
- An injection harness covering: network drop at defined points between the desktop and the machine service; a machine-service kill at defined points (`SIGKILL`); two scripted clients submitting conflicting decisions at once; human and agent edits that drift apart (CW-I03); and an ambiguous Git push (CW-G04).
- A seed and iteration count recorded for every race row.

## Acceptance
**Positive**
- Each row runs the recorded number of iterations and its invariant holds in every iteration.

**Negative**
- A stale decision is rejected and the rejection is recorded.
- Drift produces an explicit conflict. The harness fails on any silent overwrite.
- An ambiguous push shows an unknown outcome, and nothing is replayed automatically.
- A crash mid-dispatch does not replay prompts (CW-H04).
- Reject stale, unauthorized, missing or conflicting input: each row asserts its own rejection path.

## Dependencies
CW-Q01a, CW-I03, CW-G04, CW-G05a, CW-H04. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Deterministic race suite under `cargo test`, with injection points and seeds recorded.
- Real-network drop checks between the desktop and the machine service on Ubuntu.
- No check has been run for this planned item.

## Documentation impact
- [Testing and acceptance](../testing-and-acceptance.md): the desktop matrix and its invariants.
- [Consistency and recovery](../consistency-and-recovery.md): mapping from invariants to rows.
