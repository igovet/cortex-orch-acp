# CW-C07: Artifact/version-bound human acceptance separate from runtime completion

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Add a `result.accept` operation that records a human acceptance bound to one exact result version: the task, the execution, the requirements revision, the artifact, the project digest, and the result version. The operation re-reads the current observation under the runtime's operation fence and rejects any mismatch or any incomplete required evidence. Acceptance is stored in the same project store but kept separate from runtime completion. Accepting never commits, pushes, merges or releases anything.

## Requirements

CW-R04 (version checks, idempotent requests, server-side authorization) and CW-R05 (acceptance is version-bound and separate from Git actions). See [requirements](../requirements.md).

## Inputs

- [domain-model](../domain-model.md): human acceptance does not imply a commit, integration or deployment; drift invalidates or marks acceptance stale.
- [ADR 0007](../adr/0007-result-acceptance.md): version-bound acceptance, separate from runtime completion and Git push.
- [CW-C05](CW-C05.md): the current result projection and its status reasons.
- [CW-C06](CW-C06.md): stable work-item and execution identities.
- [consistency-and-recovery](../consistency-and-recovery.md): a changed result digest invalidates prior acceptance.

## Deliverables

- Acceptance record: identities, digest, requirements revision, result version, actor device, request identity and time.
- `result.accept` handler under the operation fence, with the same idempotency contract as decisions.
- Projection field `accepted_current` with a reason when false (for example `digest_drift`), while the historical acceptance remains in history.

## Acceptance

Positive:

- Accepting a current result with a matching digest records the acceptance. Runtime completion is unchanged, and both remain visible as separate facts.
- After a later drift, the historical acceptance remains, and the current projection shows `accepted_current: false` with the reason.

Negative:

- Accepting with a stale digest or a stale result version is rejected.
- Accepting while a required check is missing is rejected. Accepting while a required check is failed, not run or unverified is rejected unless the check is recorded as blocked by a technical reason and the request carries an explicit owner confirmation (CW-R40; owner decision of 2026-11-05). An ordinary failed check is never accepted.
- Accepting changes no Git reference (verified by comparing refs before and after) and starts no commit, push or merge.
- A duplicate accept with the same request identity returns the original receipt.
- A later digest change never rewrites a historical acceptance.

## Dependencies

[CW-C05](CW-C05.md) and [CW-C06](CW-C06.md).

## Validation

- `accept-digest-match`: accept a current result; acceptance and completion remain separate.
- `accept-stale-rejected`: stale digest and stale version are rejected.
- `accept-missing-check-rejected`: required check missing.
- `accept-technical-blocker-confirmed`: a check blocked by a technical reason is accepted only with an explicit confirmation; an ordinary failed check is rejected.
- `accept-no-git-side-effect`: refs and index compared before and after acceptance.
- `accept-drift-marks-not-current`: change one file after acceptance; history kept, current false.
- `accept-idempotent`: duplicate request returns the original receipt.

## Documentation impact

- [domain-model](../domain-model.md), [protocol](../protocol.md) (acceptance command and projection), [ADR 0007](../adr/0007-result-acceptance.md) (status), [ide-and-git](../ide-and-git.md) (acceptance does not commit or push).
