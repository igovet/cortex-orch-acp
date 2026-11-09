# CW-N01: Embed iroh Endpoint in the desktop service; persistent NodeId keypair in OS keyring; reject any debug listener bound to 0.0.0.0

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Give the Ubuntu machine service one long-lived iroh Endpoint whose NodeId is the stable desktop identity. The secret key lives only in the OS keyring, and no debug or diagnostic listener can be reached from all interfaces.

## Requirements
CW-R08 (secure enrollment and reconnect without extra installed software) and CW-R16 (protect credentials and device identities; reject unauthorized operations). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): embedded iroh transport; the relay forwards encrypted traffic only; no inbound ports required on either side.
- [Security design](../security.md): secrets stay in OS-protected storage; keys and tokens are never logged.
- [ADR 0002](../adr/0002-independent-machine-service.md) and [ADR 0004](../adr/0004-embedded-remote.md): the service owns connections, and closing a client is not cancellation.
- CW-F01 (pinned Ubuntu Tauri 2 shell and Rust toolchain) and CW-H02 (project registration and authenticated local RPC).
- The iroh crate version and its relay and local-discovery feature flags must be checked against the version pinned in CW-F01. No iroh code exists in this repository yet.

## Deliverables
- Endpoint bootstrap in the Rust machine service, with the iroh version pinned in the CW-F01 lock file.
- First-run keypair generation. The secret is stored through the OS keyring (Secret Service / libsecret on Ubuntu). If no unlocked keyring is available, remote mode reports `unavailable`; no plaintext key file is written.
- Explicit bind configuration. A listener-policy guard refuses startup when any debug or diagnostic listener is configured on `0.0.0.0` or `[::]`. Debug listeners are off by default.
- Status RPC that returns NodeId, relay configuration and endpoint state only.
- Redacted diagnostics: NodeId may appear; the secret key, pairing secrets and project content never do.

## Acceptance
**Positive**
- The same NodeId is returned after three service restarts and one machine reboot. Each restart is recorded.
- Remote mode starts without any router port-forwarding configured. The relay path reports its status.

**Negative**
- A debug listener configured on `0.0.0.0` or `[::]` makes startup fail with an explicit error. The socket table shows no listener on that port.
- A locked or absent keyring leaves remote mode `unavailable`. A filesystem scan of the project, config and cache directories finds no key file.
- Secret key bytes do not appear in logs, RPC responses or diagnostic exports. The check is a byte-pattern scan of captured output.
- Unauthenticated local RPC calls are still rejected. CW-H02 owns that behavior; this item adds a regression case.
- Reject revoked or expired credentials: not applicable here, because this item issues no device grants. Device revocation is covered by CW-N04.

## Dependencies
CW-H02, CW-F01. The owner gate CW-D05 is recorded; Gate M does not gate this item.

## Validation
- `cargo test` in the machine-service crate, filtered to endpoint and listener-policy tests. The exact crate and filter are defined in CW-F01 and CW-H01. Mock-keyring tests are unit fixtures, not proof of production behavior.
- On the reference Ubuntu machine: real Secret Service behavior (unlocked, locked, absent), NodeId stability across restart and reboot, and a listener listing (`ss -ltnup`). Record OS version, desktop session, keyring implementation and iroh version.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): keyring storage, unavailable state, and the listener rule.
- [Security design](../security.md): listener policy and key-storage failure behavior.
- [Roadmap](../roadmap.md): no status change until implementation begins.
