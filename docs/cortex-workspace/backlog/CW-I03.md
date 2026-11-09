# CW-I03: Hash-checked save, create and rename

**Status:** partly built: hash-checked save through the machine helper, with no lease. Create, rename and save-as remain planned · **Phase:** 3+ · **Size:** L

## Goal
Make file save, create and rename service operations. Each one is checked against the base hash or an explicit non-existence precondition. Writes replace the target atomically. Conflicts return typed results with the choices compare, reload and save-as. A granted lease from CW-I02 is required.

## Requirements
- CW-R10: writes are bounded, path-checked and protected against concurrent changes.
- CW-R16: stale and unauthorized writes are rejected.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): save requires the current content hash and a runtime-confirmed lease; conflicts offer compare, reload or save-as, never silent overwrite.
- [ADR 0006](../adr/0006-human-edit-git.md).
- CW-I01 (read and hash) and CW-I02 (lease).

## Deliverables
- Save: write a temporary file in the same directory, flush it, and rename it over the target. Return the new hash.
- Create: fails if the path exists.
- Rename: fails if the target exists. Symlinks are refused.
- Conflict result carrying the current hash and the three choices.
- Save-as: writes to a new path under the same root.

## Acceptance
Positive:
- A save with a matching base hash writes atomically and returns the new hash.
- An interrupted save (fault injected between the temporary write and the rename) leaves the original file unchanged. The temporary file is removed or reported.

Negative:
- A save with a stale base hash returns a conflict and writes nothing.
- A save without a granted lease is refused.
- Create over an existing path and rename onto an existing target are refused. Nothing is overwritten silently.
- Two devices saving the same base at the same time: exactly one succeeds, and the other receives a conflict.
- A write whose target is a symlink is refused.

## Dependencies
- [CW-I01](CW-I01.md) (read and hash), [CW-I02](CW-I02.md) (human-mutation lease).
- Gate records: CW-D05 (Gate D).

## Validation
- Rust integration tests on Ubuntu with fault injection at the rename step and a concurrent save race.
- Compare file hashes before and after each refused operation to prove nothing changed.
- Record exact commands, toolchain version and outcomes.

## Documentation impact
- [Protocol](../protocol.md) and [security](../security.md) for write commands and conflict results.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: atomic write semantics, fault injection, conflict results and a two-device race, all on the service side.
