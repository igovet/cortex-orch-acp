# CW-S07: Identified single and batch question answers

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** L

## Goal

Make durable questions answerable through an identified operation: one question or one complete batch, with exact question identities, option identities and version checks. Exactly one of several concurrent answers wins, and a winning answer wakes the run through the existing continuation path.

## Requirements

CW-R04 (idempotent, versioned commands), CW-R16 (every request is authorized) and CW-R15 (Cortex-specific views use the public adapter). See [requirements](../requirements.md).

## Inputs

- [steering-and-questions](../../features/steering-and-questions/index.md): current question semantics, for reference only.
- [CW-S05](CW-S05.md) and [CW-S06](CW-S06.md): journal, snapshot and execution binding.
- The existing question batch answer and worker wait answer transactions. They are reused, not duplicated.

## Deliverables

- Answer operation with request identity, question or batch identity, expected version, owner epoch and the answer payload.
- Multiline text kept exactly as entered. Attachments are not negotiated in this slice: the operation refuses them visibly, and the desktop control is disabled.
- After commit, the existing continuation handoff starts or resumes the run. A recorded answer does not rely on the pending question alone.
- Refusal of capacity waits and policy refusals as answerable decisions.

## Acceptance

Positive:

- The exact selected answer is delivered to the runtime.
- A batch is answered as a whole.
- Retrying the same answer with the same key returns the recorded receipt and starts no second continuation.

Negative:

- Two actors with different request identities produce one winner.
- A stale version, owner epoch, requirement revision or session is refused.
- A superseded question is refused.
- A partial batch is refused.
- Capacity and policy waits are never offered as answerable.
- An attachment is never silently dropped.

## Dependencies

[CW-S05](CW-S05.md) and [CW-S06](CW-S06.md).

## Validation

- Targeted new decision tests, with the existing `tests/test_question_batches.py`, `tests/test_acp_elicitation.py` and `tests/test_acp_host_permission_requester.py` as regression.
- Fake engines only. No paid call.

## Documentation impact

- [protocol](../protocol.md), [consistency-and-recovery](../consistency-and-recovery.md) and [ADR 0005](../adr/0005-public-contract.md).
