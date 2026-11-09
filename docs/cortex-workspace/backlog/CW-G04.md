# CW-G04: Confirmed push and ambiguous-outcome reconciliation

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Push through the Git CLI only after the owner confirms the remote, the branch, the local commit and the expected remote value. The confirmation is bound to the preview version. A lost response becomes an explicit "outcome unknown" state. Reconciliation reads the remote ref and local HEAD and then stops. A push is never force-pushed and never retried automatically.

## Requirements
- CW-R11: push requires confirmation of the remote and branch; no force push.
- CW-R07: an unknown external outcome stays unknown until independently inspected.
- CW-R04: the confirmation is version-bound and idempotent.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): confirm the remote and branch before push; never force push or auto-retry an ambiguous push.
- [ADR 0009](../adr/0009-push.md): push notifications are an opaque hint and never an authority to execute commands.
- [Consistency and recovery](../consistency-and-recovery.md): ambiguous Git pushes are not replayed.
- [CW-G03](CW-G03.md) (branch and fetch state).

## Deliverables
- Push preview: remote name and URL, source ref, target ref, commit range and expected remote SHA.
- Confirmation token bound to the preview version and the local HEAD.
- Push command with no force flags. `--force` and `--force-with-lease` are never constructed.
- Outcome classification: succeeded, rejected (non-fast-forward), failed, unknown.
- Reconciliation read of the remote ref and local HEAD after an unknown outcome.

## Acceptance
Positive:
- A confirmed push to a local bare-repository fixture succeeds. The reconciled remote SHA equals the local HEAD.

Negative:
- A simulated lost response (a fixture proxy drops the response after the server accepts the ref update) yields "outcome unknown". Only the read reconciliation runs. No second push is sent.
- A non-fast-forward push is rejected and is never forced.
- A confirmation token reused after the local HEAD changed is rejected.
- A push without confirmation is refused.
- The argv audit finds no force flags.

## Dependencies
- [CW-G03](CW-G03.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Local bare-repository fixture behind a TCP fault proxy. No real remote is contacted.
- Argv audit over the test run.
- Ref hashes compared before and after each case.
- Record exact commands, Git version and outcomes.

## Documentation impact
- [IDE and Git](../ide-and-git.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
L: confirmation binding, outcome classification, fault injection and reconciliation.
