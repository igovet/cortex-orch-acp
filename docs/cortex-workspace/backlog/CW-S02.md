# CW-S02: Shared Workspace wire subset, generated contract and scripted protocol peer

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** L

## Goal

Create the one canonical wire schema that the local slice needs: a deliberately small JSON Schema 2020-12 subset, public wire version 1.0 (independent of the product version and of ACP protocol version 1). A deterministic generator produces the TypeScript types, constants and valid and invalid fixtures. The Python validators and the Rust serde types must accept and reject the same vectors. A test-only scripted ACP peer exercises the protocol without any engine.

## Requirements

CW-R04 (versioned, idempotent, server-authorized commands), CW-R07 (structured versioned snapshots and events) and CW-R15 (generic ACP uses negotiated features only; Cortex-specific views need a public structured adapter). See [requirements](../requirements.md).

## Inputs

- [protocol](../protocol.md): planned adapter, identity and version rules.
- [ADR 0014](../adr/0014-local-service-transport.md): the public `_cortex/workspace` extension and the local transport.
- [ADR 0005](../adr/0005-public-contract.md): a typed adapter, not SQLite access and not Markdown parsing.
- [CW-S01](CW-S01.md): the toolchain pins.

## Deliverables

- `src/cortex_runtime/workspace_contract/schema.json`, committed with a fingerprint and included in the Python payload.
- `scripts/generate_workspace_contract.py` with `check` and `write` actions. It uses no model call and no host tool.
- `packages/workspace-contract/`: generated TypeScript types, constants and valid and invalid fixtures.
- `crates/workspace-protocol/`: Rust codecs with no Tauri or WebKit dependency. Commands deny unknown fields; responses accept additive fields.
- `capabilities.get` response: wire version, build identity, schema fingerprint, mapping version, implemented operation names, limits and session or connection generation. A major-version mismatch is rejected; same-major peers use the intersection of operations and known fields.
- Limits for version 1: 50 task summaries per page, 100 events or history rows per page, 64 KiB of content per response page, 1 MiB per local JSON frame, 32,000 characters per description or answer, and request identities of at most 128 characters. Public sequence numbers and entity versions are decimal strings on the wire.
- A scripted ACP peer for tests: it replays recorded frames, can inject malformed frames, and can stop responding.

## Acceptance

Positive:

- Every valid vector is accepted by the Python validator, the TypeScript fixtures and the Rust codecs.
- Capability negotiation returns exactly the intersection of operations implemented by both peers.

Negative:

- An extra command key is rejected by Rust and by Python.
- A major-version mismatch returns an explicit version error, not a partial result.
- An unrecognized enum value reaches the client as `unknown` with a warning, not as a success state.
- A schema change without a fingerprint and version change fails the drift check.
- A public sequence value above the JavaScript safe-integer range, sent as a number instead of a decimal string, is rejected.

## Dependencies

[CW-S01](CW-S01.md).

## Validation

- `python3 -B scripts/generate_workspace_contract.py check`: regenerated files equal the committed files.
- `tests/test_workspace_contract.py`: targeted pytest for valid, invalid, negotiation and drift cases.
- `cargo test -p workspace-protocol`. No WebKit is needed.

## Documentation impact

- [protocol](../protocol.md), [architecture](../architecture.md) and [ADR 0014](../adr/0014-local-service-transport.md).
