# CW-C06: Stable work-item/execution links and idempotent legacy import

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Give each user work item a stable public identifier, separate from ACP sessions and runtime executions. Import existing runtime tasks into one-to-one links with their executions and attempts, using a deterministic, idempotent import. Re-running the import changes nothing. A failed import keeps the originals intact, reports partial progress and allows a safe retry.

## Requirements

CW-R01 (stable user tasks separate from executions and sessions) and CW-R07 (repeated reads return the same stable identities and snapshots). See [requirements](../requirements.md).

## Inputs

- [domain-model](../domain-model.md): one project has many user tasks; each task has several executions; an ACP session is not a task identity.
- [migration-and-rollback](../migration-and-rollback.md): idempotent import, explicit ownership, backups, rollback point, validation and no prose-based inference.
- [CW-F04](CW-F04.md): the verified runtime identities and workspace mapping the links rely on.
- [storage](../../project/storage.md): existing migration and backup rules for the project store.

## Deliverables

- Link table: product work-item identifier to runtime task identifier, and execution identifiers to attempt references.
- Importer with a dry-run mode that lists planned links without writing.
- Per-row idempotency key, so a re-run matches existing links.
- Backup of the project store before any write, recorded with its path and hash.

## Acceptance

Positive:

- Importing a project with N runtime tasks creates N links, each one-to-one.
- Re-running the import produces the same link set with no new rows.
- A dry run lists the planned links and writes nothing.

Negative:

- A runtime task with an ambiguous mapping (two candidate work items) is reported and not linked.
- A failure partway through leaves the original runtime records unchanged, reports partial progress and allows a retry that completes the remaining links.
- A session identifier is never used as a work-item identifier.
- Import reads authoritative records only. No evidence, result or acceptance is reconstructed from transcript text.

## Dependencies

[CW-C01](CW-C01.md) and [CW-F04](CW-F04.md).

## Validation

- `import-idempotent-rerun`: second run yields an identical link set and no new rows.
- `import-dry-run-no-write`: store hash unchanged after a dry run.
- `import-partial-failure-retry`: injected failure mid-import, then retry completes.
- `import-ambiguous-mapping-reported`: ambiguous fixture is reported, not linked.
- `link-never-session-id`: link identifiers are checked against session identifiers.

## Documentation impact

- [migration-and-rollback](../migration-and-rollback.md) (import steps and rollback point), [domain-model](../domain-model.md) (identity rules).
