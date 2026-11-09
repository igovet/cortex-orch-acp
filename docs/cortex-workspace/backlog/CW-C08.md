# CW-C08: Structured steer/stop/recheck; no Paseo-only control dependency

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Expose `execution.steer`, `execution.stop` and `execution.recheck` as structured, idempotent commands in the Workspace contract. Back each command with the runtime's existing operations: the durable steering event, cancellation, and recheck after a foreign change. Each command must work without the Paseo plugin. Pause and resume are not advertised until the runtime has a real implementation and a capability test. ACP turn cancellation is not treated as a safe pause.

## Requirements

CW-R02 (actions express intent and never rewrite runtime facts), CW-R04 (version checks, idempotent requests, server-side authorization), CW-R15 (negotiated features only; Cortex-specific controls use the public adapter) and CW-R30 (Steer's lifecycle is visible to the owner; Queue stays client-side). See [requirements](../requirements.md).

## Inputs

- [protocol](../protocol.md): planned commands, identity and expected-version fields.
- [steering-and-questions](../../features/steering-and-questions/index.md) and [failures-and-cancellation](../../features/failures-and-cancellation/index.md): existing steering, cancellation and disconnect semantics.
- `policy.steer` in `src/cortex_runtime/orchestration/policy.py` (lines 61-79): steering is a durable `USER_STEERING` event; it returns `False` for a cancelled task and for a second steer from the same identity. The docstring says it changes no grants, but the event payload records `deny_write` from parsed text. This is an open point to resolve in source before the contract states the effect of a steer.
- `TaskMachine.recheck_after_foreign_change` in `src/cortex_runtime/orchestration/state.py`: the existing recheck path.
- [CW-C03](CW-C03.md) and [CW-C06](CW-C06.md): decision idempotency and stable execution identities.

## Deliverables

- Three command handlers with request identity, target identity and expected version. Each returns a typed receipt (accepted, queued, applied, refused, duplicate).
- Typed refusals in place of the current boolean returns for a cancelled task and a duplicate steer.
- Recheck creates a new execution linked to the same work item; it does not overwrite the old one.
- Stop semantics documented: which owned processes stop, and that an ambiguous stop is reported as unknown.
- Resolution note for the `deny_write` open point, with the verified effect of a steer.
- Steer lifecycle per message identity: captured, then applied at a boundary with the plan revision that carries it, or refused. This is the source of the Steer states in [CW-U12](CW-U12.md). Queue is held by the client and does not use this command.

## Acceptance

Positive:

- A steer queues a durable intent that appears in the timeline; the receipt is returned.
- A steer that is applied reports the applied state with the plan revision that carries it.
- A stop reaches a terminal state and stops the owned child processes.
- A recheck produces a new execution linked to the same work item, and the earlier execution stays unchanged.

Negative:

- A steer to a cancelled task is refused with a typed reason, not a silent `false`.
- A second steer from the same identity returns an explicit duplicate receipt, not silence.
- A steer does not change effective grants unless the resolved `deny_write` behavior is documented and tested to do so.
- A stop whose outcome is ambiguous is reported as unknown and is not retried automatically.
- `execution.pause` and `execution.resume` do not appear in the capability list.

## Dependencies

[CW-C03](CW-C03.md) and [CW-C06](CW-C06.md).

## Validation

- `steer-durable-queue`: queued steer visible in the timeline with its receipt.
- `steer-duplicate-explicit`: second steer returns the duplicate receipt.
- `steer-cancelled-refused`: typed refusal for a cancelled task.
- `stop-owned-children`: owned process groups are stopped; ambiguous stop reports unknown.
- `recheck-new-execution`: a new execution is created; the old one is unchanged.
- `pause-not-advertised`: the capability list omits pause and resume.

## Documentation impact

- [protocol](../protocol.md) (command table), [steering-and-questions](../../features/steering-and-questions/index.md) only after the contract ships, [failures-and-cancellation](../../features/failures-and-cancellation/index.md) (stop semantics).
