# CW-C01: Versioned schemas, capabilities and generated TS/Python contract fixtures

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Define the versioned public Workspace contract once, from a single machine-readable schema source. The contract covers the protocol version with major and minor numbers, the `capabilities.get` response, command envelopes (request identity, target identity, operation, payload digest, expected entity versions and owner epoch where relevant), and error and unknown-state enumerations. Generate or check TypeScript and Python fixtures from that source, and check the Rust service's types against it, so the runtime adapter and both clients share one wire shape.

## Requirements

CW-R04 (versioned, idempotent, server-authorized commands), CW-R07 (structured versioned snapshots and events) and CW-R15 (generic ACP uses negotiated features only; Cortex-specific views need a public structured adapter). See [requirements](../requirements.md).

## Inputs

- [protocol](../protocol.md): planned adapter, command identity and version checks, and error semantics.
- [domain-model](../domain-model.md): distinct task, execution, decision, result, artifact and device identities.
- [ADR 0005](../adr/0005-public-contract.md): a versioned typed adapter, not SQLite access and not Markdown parsing.
- Existing runtime vocabularies that the contract must map explicitly, not silently: `runtime_tasks.state`, `runtime_nodes.status` and `stage` (`src/cortex_runtime/orchestration/state.py`); `task_orchestration.state`, `wait_reason` and `task_events.event_type` (`src/cortex_runtime/delegation/events_store.py`).
- [CW-F04](CW-F04.md): the verified path and lifecycle mapping that identities must refer to.

## Deliverables

- One schema source (the format is chosen and recorded in this item) with the protocol version rule.
- Capability list: each name corresponds to one implemented operation. Generic ACP features and Cortex extensions are listed separately.
- Generated or checked fixtures for TypeScript, Python and the Rust service types, with one valid and several invalid samples per message.
- Documented rule: any runtime enum value the contract does not recognize is surfaced as `unknown` with a warning.
- Management operations for orchestration settings ([CW-U07](CW-U07.md), [CW-R41](../requirements.md)): each settings surface (`user_defaults`, `connections`, `routing`, `project_ceiling`, `mcp_servers`, routing profiles) carries its own `sha256:` revision and is written as one request; an `mcp_servers` write carries up to 64 operations. Typed conflict and unknown-outcome responses.
- New capability names, each advertised only when implemented: tool listing from an unsaved server draft (sandboxed start, `initialize` and `tools/list` only, bounded, fixed failure codes, no stored secrets); a read-only route preview that accepts draft rules and writes no audit record; built-in routing facts (role tiers, access classes, fallback order); observed quota window labels with their percentages; machine defaults for access mode and checkout.

## Acceptance

Positive:

- A TypeScript fixture and the Python fixture both accept every valid sample for every message.
- `capabilities.get` lists only operations the build implements.
- A client on the same major version and a higher minor version negotiates successfully; a different major version gets an explicit version error.

Negative:

- An unrecognized runtime state reaches the client as `unknown`, not dropped and not mapped to "done".
- A command without a request identity or without its expected version is rejected by schema validation.
- A command carrying an unknown field is rejected (commands are strict; responses may add fields in a minor version).
- A capability that was not negotiated cannot be invoked; the call fails explicitly.
- Changing the schema without a version bump fails the drift check.
- A tool-listing request never sends `tools/call`. Its response carries no raw server output and no stored secret, and a failed listing changes no stored grant.

## Dependencies

[CW-F04](CW-F04.md). Gate D ([CW-D05](CW-D05.md)) is recorded; Gate M does not gate this item.

## Validation

- `contract-fixtures-check`: regenerated fixtures equal the committed fixtures.
- `contract-negative-samples`: every invalid sample is rejected with the expected error code.
- `version-negotiation`: same-major, cross-minor and cross-major cases.
- `unknown-enum-surfaces-unknown`: runtime enum values not in the contract produce `unknown`.

## Documentation impact

- [protocol](../protocol.md) (final field names and error codes), [domain-model](../domain-model.md) (identity and state mapping), [ADR 0005](../adr/0005-public-contract.md) (status after the contract is accepted).
