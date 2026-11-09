# CW-D01: Shared realistic desktop fixtures and scenario script; no private data

**Status:** mock built in the browser prototype (`cw-fixtures-v3`); unqualified; Gate D passed 2026-11-05 · **Phase:** 1 · **Size:** S

## Goal
Shared realistic desktop fixtures and scenario script; no private data.

## Requirements
CW-R13, CW-R14; see [requirements](../requirements.md).

## Inputs
[Architecture](../architecture.md), [testing](../testing-and-acceptance.md), and the dependencies below. Validate current source and tool support before implementation.

## Deliverables
A versioned synthetic fixture set and scenario for the single revised direction: one local machine with projects, an optional offline remote machine with projects, standalone and sprint tasks, and tasks using either the current checkout or a dedicated worktree. Include lifecycle, archive, conflict and title-generation failure cases; preserve deterministic large-history coverage.

## Acceptance
- The loader deterministically models machine → projects → optional sprints → tasks and exposes blocked, stale, offline and conflict cases; no machine is derived from a project.
- No production backend, credentials or private fixture data; mock outcomes are not runtime evidence.

## Dependencies
CW-D06a, CW-D06b, CW-D06c, CW-D07. The design gate passed on 2026-11-05; implementation waits for these dependencies.

## Validation
Fixture privacy and determinism: load twice and compare counts and stable IDs; inspect fixture content for private or live data. Record commands, environment, outcomes and unrun cases.

## Documentation impact
Update the relevant planned contract and [roadmap](../roadmap.md) when behavior or qualification changes.

## Status record

- Round one (historical): in `prototypes/cortex-workspace/desktop/`, `src/fixtures.ts` (revision `cw-fixtures-v1`, seed 13014), `src/store.ts`, `src/scenarios.ts`, and fixture tests. The fixture tests cover equal route data, deterministic counts and stable IDs, required cases and a privacy scan. The round-one routes are no longer served by the app; their source remains in the tree.
- Revised fixtures (2026-11-05 build record): `prototypes/cortex-workspace/desktop/src/workspace/adapter/fixtures.ts`, revision `cw-fixtures-v3`, seed 13014. It models one local machine and one offline remote machine, eight projects in total, standalone and sprint tasks, main-checkout and worktree bindings, archive cases, conflict, title-generation failure and large-history identities (500 tasks, 10,000 events). The proposal's P0 list names four projects; eight are kept for compatibility with the existing routes. Fixture tests cover determinism and stable identities. No machine is derived from a project.
- Verified 2026-11-04: `npm run typecheck`, `npm test` (39 tests in 5 files, all passing) and `npm run build` exited 0. See [desktop comparison](../design/desktop-comparison.md), section 2.5.
- Open: the native Tauri build was not run (Cargo and rustc are not installed; the owner chose the browser only). Mock outcomes are not runtime evidence.
