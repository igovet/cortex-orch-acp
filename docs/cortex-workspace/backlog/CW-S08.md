# CW-S08: Rust service ownership, authenticated local RPC and child supervision

**Status:** built; no-paid checks pass; the different-OS-user peer check is unverified · **Phase:** 3+ · **Size:** L

## Goal

Build the per-user Rust service that owns the Cortex ACP child process and a local Unix domain socket. A single exclusive instance runs per user. Both ends authenticate by peer credentials. The service never reads Cortex SQLite and never parses reports or transcripts to infer state.

## Requirements

CW-R06 (the service owns the connection and child lifetime), CW-R15 (explicit project activation) and CW-R16 (every request is authorized). See [requirements](../requirements.md).

## Inputs

- [ADR 0002](../adr/0002-independent-machine-service.md): closing a client is not cancellation.
- [ADR 0014](../adr/0014-local-service-transport.md): the local transport decision.
- [CW-H01](CW-H01.md) and [CW-H02](CW-H02.md): the full items. This item implements the narrow slice subset.

## Deliverables

- `crates/workspace-service/`: Tokio service, project locator registry (canonical root, display name, activation record), and the authenticated socket.
- Private runtime directory `$XDG_RUNTIME_DIR/cortex-workspace/` with mode 0700, socket mode 0600 and an exclusive `service.lock`. The lock is an `flock` held on an open descriptor for the service lifetime, not a PID file. Only the lock winner may replace its own stale socket.
- Registration rejects symlinks, `.cortex`-private roots and paths that escape the canonical root. Registration does not start Cortex. Activation is explicit and recorded.
- Child supervision: an explicit release-owned executable, canonical working directory, an allowlisted inherited environment, continuous stdout and stderr drain in independent tasks, exact reply correlation, and exit observation. Protocol errors or queue overflow invalidate readiness.
- Closing a client detaches only its subscription. It sends no `session/cancel`, closes no child stdin and stops no child.
- Explicit service stop: refuse new commands, request runtime cancellation, close stdin, wait a bounded time, then terminate only owned children.
- Restart after a crash: reread locators, take public snapshots from a fresh child, and mark uncertain executions interrupted or unknown. Never resend a start or a prompt.

## Acceptance

Positive:

- A second instance is refused by the lock.
- Registration leaves the project inactive; activation records actor and time.
- Detaching a client leaves its child running. Queue overflow leads to a resynchronization.

Negative:

- A connection from another OS user is refused. If no suitable second account exists, this check is recorded as unverified; a mocked credential function does not count as a pass.
- A symlink, `.cortex`-private or outside-root registration is refused.
- No TCP or loopback listener is opened.
- An unknown outcome is never replayed automatically.

## Dependencies

[CW-S02](CW-S02.md), [CW-S04](CW-S04.md), [CW-S05](CW-S05.md), [CW-S06](CW-S06.md) and [CW-S07](CW-S07.md).

## Validation

- `cargo test -p workspace-service` with a fake ACP process, covering second-instance refusal, peer checks, symlink refusal, reply correlation, exit observation, detach and overflow.
- Integration with the real Agent and an injected fake-backed runtime. No paid call.

## Documentation impact

- [security](../security.md), [architecture](../architecture.md), [protocol](../protocol.md) and [ADR 0014](../adr/0014-local-service-transport.md).
