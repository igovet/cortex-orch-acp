# CW-T01: Service-owned PTY sessions

**Status:** local project slice built; task/worktree sessions and wider qualification remain planned · **Phase:** 3+ · **Size:** L

The local slice uses portable-pty 0.9.0 and xterm.js 6.0.0 with fit addon 0.11.0 (DOM renderer). It has one owner shell per registered project, including in task views; the initial cwd is the project folder, not the task worktree. Ctrl+J opens/hides it. Bounds, same-user authority and cleanup limits are in [security](../security.md#owner-terminal-built-local-linux). This does not qualify remote terminals or the CW-F03 GPU fixture.

## Goal
Make the Rust service create, attach, resize and stop PTY sessions through the portable-pty crate. Sessions belong to the service, so they survive closing the desktop window. A stop terminates the owned process group. Resource limits bound the number of sessions and the retained output.

## Requirements
- CW-R10: a bounded, authorized terminal surface.
- CW-R06: the service owns child sessions and survives window closure. Crash reconciliation is in CW-H04.

See [requirements](../requirements.md).

## Inputs
- [Architecture](../architecture.md): the service owns PTYs; clients are views.
- [Consistency and recovery](../consistency-and-recovery.md): terminal commands are not replayed after a crash.
- [Security](../security.md): a remote terminal carries real execution authority, so every request is authorized.
- [CW-F03](CW-F03.md): the xterm.js fixture. Confirmation of portable-pty behaviour is part of that item and is not assumed here.

## Deliverables
- Session manager: create (working directory inside the registered root only), attach and detach streams, resize, stop.
- Output ring buffer with a byte cap. Reattaching replays the retained tail.
- Stop: SIGTERM to the process group, a grace period, then SIGKILL.
- Limits: sessions per project, output cap, and documented child resource limits.

## Acceptance
Positive:
- A session started from the desktop continues after the window closes. Reattaching shows the retained output.
- An explicit stop ends the shell and its child processes. A process-group check confirms it.

Negative:
- A create request with a working directory outside the root is refused.
- A session beyond the limit is refused with a typed error.
- A runaway output producer does not grow service memory beyond the buffer cap.
- After a service restart, a previously running session is reported as lost or unknown, and its command is not re-run.
- An unauthorized attach is refused.

## Dependencies
- [CW-I02](CW-I02.md), [CW-F03](CW-F03.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Ubuntu integration tests using a real shell and child processes. Process groups are checked with `ps -o pid,pgid`.
- Memory-cap test with a high-output producer.
- Rendered xterm.js check through the CW-F03 fixture with a reattach.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Architecture](../architecture.md) and [security](../security.md) once the terminal boundary is implemented.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: a process-owning session manager with limits, reattach semantics and authorization.
