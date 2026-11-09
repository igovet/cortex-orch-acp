# CW-I02: Human-mutation lease and writer quiescence

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Provide a runtime-coordinated lease that lets a human edit a path scope only after the runtime confirms that no model writer is active in that scope. The lease is released, expires, or is dropped when its device is revoked. The lease replaces visual pause controls with an enforced runtime state.

## Requirements
- CW-R10: human edits and agent changes are protected against each other.
- CW-R16: stale, revoked or unauthorized requests are rejected.

See [requirements](../requirements.md).

## Inputs
- [ADR 0006](../adr/0006-human-edit-git.md): runtime-confirmed quiescence and hash or version checks; never a visual pause.
- [IDE and Git](../ide-and-git.md): saves require a runtime-confirmed human mutation lease.
- [Consistency and recovery](../consistency-and-recovery.md): stale owner epochs and device races need executable tests.
- CW-F04 (current workspace ownership) and CW-C08 (structured stop and steer, used to quiesce writers).

## Deliverables
- Lease record: identifier, project, path scope, holder device, owner epoch, expiry, and state (requested, quiescing, granted, released, expired, dropped).
- Writer-quiescence query over the runtime's real writer state for the scope.
- Typed refusal reasons: writer active, scope conflict, stale epoch, device revoked.
- Expiry, renewal and drop-on-revocation rules.

## Acceptance
Positive:
- A request made while a model writer is active in the scope returns "quiescing". After the runtime confirms zero writers, the lease becomes granted.
- Releasing the lease returns the scope to normal writer scheduling.

Negative:
- A UI-side pause or timer without runtime confirmation never grants the lease.
- Two devices requesting overlapping scopes: exactly one lease is granted. The other is refused with a scope-conflict reason.
- An expired lease or a stale owner epoch is rejected for every write.
- Revoking a device drops its lease, and its later write attempts are refused.

## Dependencies
- [CW-F04](CW-F04.md) (workspace ownership), [CW-C08](CW-C08.md) (structured stop and steer).
- Gate records: CW-D05 (Gate D).

## Validation
- Integration tests against the runtime's writer state, using scripted fake-ACP writer fixtures. These results do not prove production engine behaviour, and the report must say so.
- Concurrency test with two requesters on overlapping scopes.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Consistency and recovery](../consistency-and-recovery.md) and [ADR 0006](../adr/0006-human-edit-git.md) consequences once implemented.
- [Protocol](../protocol.md) for lease commands.

## Size
L: a lease state machine tied to real runtime writer state, with typed refusals and revocation.
