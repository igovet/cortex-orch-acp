# CW-Q05: Desktop migration/rollback rehearsal; no production Paseo mutation

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Rehearse upgrade and rollback of the machine service data (project registration, allow-list, outbox cursor) and the desktop client cache. The Android client cache is covered by CW-Q04b, gated by Gate M. Run the rehearsal on copies only, so the owner's real project store and the production Paseo installation are never written.

## Requirements
CW-R12 (qualify before release) and CW-R14 (evidence, not claims). See [requirements](../requirements.md).

## Inputs
- [Migration and rollback](../migration-and-rollback.md) and [consistency and recovery](../consistency-and-recovery.md).
- [Architecture](../architecture.md): the new public adapter reads through the project read model, not internal SQLite.
- CW-Q04a (installer and update evidence) and CW-C06 (stable links and idempotent legacy import).

## Deliverables
- A rehearsal script, or a runbook step list, that operates on a copy of the store and a copy of the desktop client cache.
- A pre-migration and post-migration checksum record for each store file.
- A rollback procedure exercised against the same copies.
- A guard that refuses any path under the production Paseo home (`~/.paseo`) or the production Cortex store.

## Acceptance
**Positive**
- Migration on the copy completes. The post-migration state matches the expected records from CW-C06.
- Rollback restores the pre-migration checksums exactly.

**Negative**
- The source store's checksum is unchanged before and after the rehearsal.
- A migration failure leaves the copy unchanged, and the failure is recorded.
- A path under the production Paseo home is refused before any write.
- No success is inferred from a migration that produced no records.

## Dependencies
CW-Q04a, CW-C06. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Rehearsal run on copies, with checksums and logs recorded. The rehearsal host, date and tool versions are recorded.
- The path-guard test runs against a deliberately forbidden path.
- No check has been run for this planned item.

## Documentation impact
- [Migration and rollback](../migration-and-rollback.md): rehearsal steps and results format.
