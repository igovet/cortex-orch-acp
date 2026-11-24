# CW-N04: Device allow-list and immediate revocation enforced at the QUIC/ALPN accept step; force-close-on-revoke test

**Status:** Partly implemented (no-paid tests only, not qualified): a device registry saved before Allow is acknowledged, refusal of application RPC connections from unlisted devices, revocation that closes live connections at once and does not restore access if the registry write fails, and a damaged registry that keeps remote access off. Remaining: a per-device scope (read-only versus full control), the monotonic allow-list version counter, a revocation event through the public outbox, and a measured revoke budget on a real network ([security](../security.md#remote-access-built-not-qualified)) · **Phase:** 3+ · **Size:** M

## Goal
Make the desktop's local allow-list the authority for remote device authorization. The host checks each application RPC connection against it before exchanging application data. When remote access is enabled, unlisted devices can still use pairing and the constants-only version probe; neither grants RPC access on its own. Revocation closes live RPC sessions immediately and rejects RPC reconnects.

## Requirements
CW-R08 (revoke access and reconnect securely) and CW-R16 (reject stale, revoked or unauthorized commands). See [requirements](../requirements.md).

## Inputs
- [Security design](../security.md): revoke immediately, close active sessions and reject future operations.
- [Consistency and recovery](../consistency-and-recovery.md): reject stale entity versions, old owner epochs and revoked device identities.
- [Protocol](../protocol.md): version checks, idempotent requests and typed rejections.
- CW-N03 (enrollment writes the allow-list entry) and CW-C02 (public outbox, which carries the revocation event).

## Deliverables
- A local allow-list store, written atomically. Each entry holds NodeId, label, scope, created and revoked timestamps. The store carries a monotonically increasing allow-list version.
- An accept hook on the endpoint. Application RPC connections from NodeIds not on the list close before application data is exchanged. Unknown ALPNs are refused.
- Scope enforcement at the RPC layer for listed devices. Read-only scope cannot submit writes or decisions. This integrates with the RPC authorization owned by CW-H02.
- A revoke command. It removes the entry, bumps the version, closes every live connection from that NodeId and emits a revocation event through the public outbox. It is idempotent.

## Acceptance
**Positive**
- A listed device connects and passes scope checks.
- Revoking a device closes its live session within a measured budget. The actual measured time is recorded. A new RPC connection attempt is refused, and no RPC bytes are read.

**Negative**
- An unlisted NodeId can reach pairing and the constants-only version probe, but its application RPC connection is refused before any RPC bytes are read. The log entry contains the NodeId only.
- A revoked NodeId that reconnects for RPC is rejected. Its pending decision submissions return an explicit revoked error (CW-R04).
- A read-only scope cannot submit a decision or write.
- A revoke that races an in-flight decision produces exactly one of two outcomes: the decision is applied before revoke with its version recorded, or it is rejected. No partial state remains.
- A corrupted allow-list file fails closed: no device is allowed, and an explicit error is shown.
- Force-close: after revoke is acknowledged, an active event stream from the revoked device delivers no further events.
- Reject revoked or expired credentials: this item is the enforcement point for that rule.

## Dependencies
CW-N03, CW-C02. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Rust integration tests with two local endpoints and fixture NodeIds. They cover accept rejection, live-session force close, revoke idempotency and the decision race.
- Repeated runs of the race test, with the iteration count and seed recorded.
- Real-network revoke on Ubuntu belongs to CW-Q02a; the Android part belongs to CW-Q02b.
- No check has been run for this planned item.

## Documentation impact
- [Security design](../security.md): revocation semantics and the accept-step rule.
- [Remote access](../remote-access.md): revocation and host-key rotation procedure.
- [Public contract](../protocol.md): revocation event shape.
