# CW-H02: Project registration and authenticated local RPC; explicit Cortex activation

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal

Let the owner register canonical project roots with the machine service, and expose a local RPC authenticated for the local user only. The service stores only project locators and connection settings, never a replica of project tasks or evidence. Registering a project does not activate Cortex. Activation is a separate, explicit owner action that is recorded. Remote device pairing is out of scope here and belongs to the enrollment items.

## Requirements

CW-R06 (the service owns the connection and child lifetime), CW-R15 (Cortex-specific views require explicit activation and the public adapter) and CW-R16 (every request is authorized; unauthorized commands are rejected). See [requirements](../requirements.md).

## Inputs

- [security](../security.md): every request is authorized; path restrictions; traversal and symlink defence.
- [architecture](../architecture.md): explicit project activation is required; no home-wide replica.
- [remote-access](../remote-access.md): device pairing is a separate, later path.
- [CW-H01](CW-H01.md) (service process) and [CW-C01](CW-C01.md) (contract, capabilities and version negotiation).

## Deliverables

- User-level registry of project locators (canonical root, display name, activation state, recorded time).
- Local RPC on a Unix domain socket with peer-credential checks, so only the same OS user can connect.
- Registration, listing and explicit activation commands, each with a request identity and a typed result.

## Acceptance

Positive:

- Registering a project lists it as registered and not active.
- Activation requires its own command, and the recorded activation shows the actor and the time.
- A request from the same OS user with a valid registration succeeds.

Negative:

- A connection from another OS user is rejected by the peer-credential check.
- A registration of a path that is a symlink, a path outside its canonical root, or a `.cortex`-private path is rejected.
- No network listener is opened by this item. Any TCP listener, including on loopback, is out of scope and fails the check.
- A registered project's tasks, evidence or transcripts are not stored in the service's user-level state.

## Dependencies

[CW-H01](CW-H01.md) and [CW-C01](CW-C01.md).

## Validation

- `register-not-activate`: registration leaves the project inactive.
- `activation-recorded`: activation writes actor and time.
- `peer-credential-rejected`: a connection from a different OS user (test harness) is refused.
- `root-symlink-and-private-path-rejected`: symlink, outside-root and `.cortex` paths refused.
- `no-network-listener`: the service opens only the Unix socket.

## Documentation impact

- [security](../security.md), [architecture](../architecture.md), [protocol](../protocol.md) (registration and activation commands).
