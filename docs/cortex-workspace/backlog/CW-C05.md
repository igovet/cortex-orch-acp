# CW-C05: Result/evidence completeness and digest-current projection

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Publish a versioned result projection that binds one execution's result to its requirements revision, its workspace or artifact digest, its check outcomes with retained receipts, its required reviews, and its unrun or unknown items. Compute a current or stale status by comparing the digest stored with the result against the digest the runtime observes now. An incomplete or unavailable observation yields "unknown", never "current".

## Requirements

CW-R05 (results expose evidence, missing checks and the exact revision), CW-R07 (unknown remains unknown). See [requirements](../requirements.md).

## Inputs

- [domain-model](../domain-model.md): a result identifies evidence, check outcomes and the workspace or artifact digest; acceptance attaches to that digest and drift marks it stale.
- [consistency-and-recovery](../consistency-and-recovery.md): a changed result digest invalidates prior acceptance.
- Existing runtime evidence and gates: `src/cortex_runtime/delegation/evidence.py`, and the stale-verification comparison described in [workspaces-and-integration](../../features/workspaces-and-integration/index.md) (revision binding and re-verification).
- Existing digest capture: `src/cortex_runtime/delegation/revision.py`. Confirm the digest scope in source before relying on it.
- [CW-C04](CW-C04.md): the typed execution projection this result hangs from.

## Deliverables

- Result projection: execution identity, requirements revision, digest, check list (outcome, receipt identity, required flag), reviews, unrun and unknown items, creation time and current or stale status.
- Completeness rule: a check counts as passed only with a retained receipt.
- Current or stale computation with a machine-readable reason (for example `digest_drift`, `requirements_changed`, `observation_incomplete`).

## Acceptance

Positive:

- A result whose required checks all passed and whose digest matches the current observation shows `current: true`.
- After one file changes, the same result shows `current: false` with reason `digest_drift`.
- A missing required check appears in the missing list, and the result shows `complete: false`.

Negative:

- An incomplete, truncated or failed project observation yields status `unknown`, never `current`.
- A check recorded as passed without a retained receipt is rejected as evidence.
- A result whose requirements revision differs from the current one shows stale status.
- A result from an earlier digest is not rewritten when the current digest changes.

## Dependencies

[CW-C04](CW-C04.md).

## Validation

- `result-digest-drift`: change one file after the result; status becomes stale with the reason.
- `observation-incomplete-unknown`: an injected incomplete observation yields `unknown`.
- `missing-check-listed`: a required check with no receipt appears in the missing list.
- `receipt-required-for-pass`: a pass without a receipt is rejected.

## Documentation impact

- [domain-model](../domain-model.md) and [protocol](../protocol.md) (result projection and status reasons); [consistency-and-recovery](../consistency-and-recovery.md) (digest-current rule).
