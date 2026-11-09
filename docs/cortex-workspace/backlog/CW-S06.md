# CW-S06: Idempotent explicit start and read-only result projection

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** L

## Goal

Let the owner start a saved draft explicitly. One start creates one runtime execution through the existing prompt and runtime path, with a durable receipt and a crash-safe claim before dispatch. Results are projected as content with their review and verification status, and are labelled as runtime results that are not accepted or merged.

## Requirements

CW-R04 (idempotent commands), CW-R06 (the service owns the ACP connection and child lifetime) and CW-R07 (structured events and results). See [requirements](../requirements.md).

## Inputs

- [CW-S04](CW-S04.md) and [CW-S05](CW-S05.md): draft identity, journal and snapshot.
- [ADR 0007](../adr/0007-result-acceptance.md): acceptance is separate from runtime completion.
- [consistency-and-recovery](../consistency-and-recovery.md): unknown outcomes and no automatic replay.

## Deliverables

- Start command with a request identity and states prepare, claimed and dispatched. Only one nonterminal execution is allowed per project checkout; a second start is refused while the first is active or unresolved.
- Trusted adapter-supplied source identity passed through the prompt capture boundary. Generic ACP behavior is unchanged for other clients.
- Result projection: result identity and version, execution binding, text, evidence references and digests, checks with explicit outcomes, and an optional observed workspace digest. The display label is "Runtime result; not accepted or merged". Ready-to-merge and integration controls stay unavailable.
- Drift between the observed result and the current checkout invalidates the observed binding.

## Acceptance

Positive:

- Two start requests with the same key return the same receipt.
- A fake-backed production orchestration reaches review and verification, and its result references resolve.
- Both an answer-only result and a work result are projected.

Negative:

- A start with a different payload under the same key conflicts.
- A crash before or after the dispatch claim leaves the execution unknown; it is never replayed automatically.
- An `end_turn` stop is not treated as completed work.
- A runtime COMPLETED result is never described as accepted.

## Dependencies

[CW-S04](CW-S04.md) and [CW-S05](CW-S05.md).

## Validation

- Targeted new adapter tests with fake engines only. No paid call.
- Regression: `tests/test_acp_elicitation.py` and `tests/test_acp_host_permission_requester.py`.

## Documentation impact

- [protocol](../protocol.md), [domain-model](../domain-model.md), [consistency-and-recovery](../consistency-and-recovery.md).
