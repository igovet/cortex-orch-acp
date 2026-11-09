# CW-S03: Promote the accepted Workspace presentation behind an asynchronous production seam

**Status:** built; no-paid checks pass · **Phase:** 3+ · **Size:** L

## Goal

Move the accepted desktop presentation (layout, navigation, theme preferences, keyboard behavior and components) into shared packages, and replace the synchronous mock port in the presentation path with a seam that can be real or demo. The production path never selects the mock backend. Demo data stays in a separate, labelled entry.

## Requirements

CW-R13 (one revised Paseo-style desktop direction, accepted at Gate D for the clickable prototype). See [requirements](../requirements.md) and [desktop approval](../design/desktop-approval.md).

## Inputs

- [desktop approval](../design/desktop-approval.md) and [DESIGN](../DESIGN.md): the accepted direction, theme and tokens.
- The clickable prototype in `prototypes/cortex-workspace/desktop/`, which is mock-only.
- [CW-S02](CW-S02.md): the wire types that the view models consume.

## Deliverables

- `packages/workspace-ui/`: promoted components, CSS, icons, router and theme preferences, with history preserved where practical.
- `packages/workspace-domain/`: pure presentation invariants and DTO-to-view-model mapping. It never executes lifecycle transitions.
- Port with `mode: "real" | "demo"`, asynchronous `query(operation, parameters)` and `command(envelope)`, and `subscribe(listener)` for confirmed projection updates and connection state. The store installs snapshots by version.
- Mock mutations, simulator, autoplay, synthetic fixtures, mock title generation and state-changing mock commands stay in the prototype. The demo entry shows a persistent banner: "Demo — simulated data, no real actions".
- A missing capability disables the matching control.

## Acceptance

Positive:

- Accepted navigation, layout, theme and keyboard behavior are unchanged, verified by the existing presentation tests.
- A command receipt does not change displayed state until a confirmed projection update arrives.
- A stale snapshot is rejected by its version.

Negative:

- The production bundle imports no simulator, autoplay or mock-mutation module; an import-boundary test fails otherwise.
- A real backend failure never selects the demo backend.
- FAILED and CANCELLED never map to "done", and COMPLETED never maps to "accepted".

## Dependencies

[CW-S02](CW-S02.md).

## Validation

- TypeScript typecheck and Vitest for the shared packages, including the import-boundary test.
- Vite production build of the desktop entry. No WebKit is needed. Rendered checks are not claimed by this item.

## Documentation impact

- [DESIGN](../DESIGN.md), [SCREENS](../SCREENS.md) and [architecture](../architecture.md), where the production seam changes the documented boundary.
