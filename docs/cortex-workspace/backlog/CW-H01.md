# CW-H01: Single machine-service instance and ACP child ownership independent of UI

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Build the Rust machine service (tokio) as a separate per-user process, not part of the desktop UI. It is the single owner of the Cortex ACP stdio child processes. It enforces one running instance per user with an exclusive lock, supervises each child (spawn, continuous stdout drain, exit observation), and keeps children alive when the UI closes. It is installed as a `systemd --user` unit. Stopping the service is an explicit action with a stated consequence. A host restart or power loss is not claimed as survived.

## Requirements

CW-R06 (the machine service survives window closure, owns child sessions and reconciles crashes without replaying prompts) and CW-R07 (structured snapshots and events are produced from the service's view). See [requirements](../requirements.md).

## Inputs

- [architecture](../architecture.md): the service owns ACP child lifetime; it survives closing the UI, not machine shutdown.
- [ADR 0002](../adr/0002-independent-machine-service.md): a UI-owned ACP process would couple execution to window lifetime.
- [remote-access](../remote-access.md): the machine must be running and reachable; power loss is not survivable.
- Current ACP entry points: `src/cortex_acp.py` and `src/cortex_runtime/acp/`. Confirm how EOF on stdio is handled today before designing the supervisor. The product docs describe EOF handling in [failures-and-cancellation](../../features/failures-and-cancellation/index.md).
- [CW-F01](CW-F01.md) (toolchain) and [CW-F04](CW-F04.md) (verified lifecycle and paths).

## Deliverables

- Service binary skeleton with an exclusive per-user lock file and a clear refusal message when a second instance starts.
- Child registry: one entry per owned child, with its state (starting, running, exited, unknown).
- Continuous stdout drain loop per child; a drain stall is detected and reported.
- `systemd --user` unit file and an explicit stop command that states what stops.

## Acceptance

Positive:

- Closing the desktop UI leaves the child running; its PID is unchanged and its output is still drained.
- A second service instance refuses to start while the first holds the lock.
- An explicit service stop stops its children and reports that it did so.

Negative:

- A second UI or a second service instance cannot become a second owner of an ACP child.
- A child whose stdout stops being drained is reported as a failure, not left silently blocked.
- Killing the UI does not terminate children.
- The service writes no runtime state directly; all runtime changes go through the runtime's handlers.
- The item makes no claim of survival across host restart or power loss.

## Dependencies

[CW-F01](CW-F01.md) and [CW-F04](CW-F04.md).

## Validation

- `ui-close-child-survives`: close the client; child PID and output stream continue.
- `single-instance-lock`: second start refused.
- `stdout-drain-continuous`: sustained child output is consumed; a deliberately stalled drain is reported.
- `service-stop-explicit`: stop stops children and reports the consequence.
- `no-direct-runtime-write`: code check that the service has no direct write path to the project store.

## Documentation impact

- [architecture](../architecture.md), [consistency-and-recovery](../consistency-and-recovery.md) (child lifetime), [platforms](../platforms.md) (service on Ubuntu), [roadmap](../roadmap.md).
