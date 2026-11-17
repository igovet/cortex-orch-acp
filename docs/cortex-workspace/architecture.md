# Architecture

**Status: planned product architecture. The local desktop slice in the last section is built with its no-paid checks; no platform is qualified for release.**

Desktop Tauri 2 / SolidJS and Android clients (React Native is provisional until the CW-F02 go/no-go) are views, not authorities. A separate Rust/Tokio service on the Ubuntu machine owns authenticated connections, ACP child lifetime, files, Git, PTYs and language servers. It survives closing the UI, not machine shutdown. Cortex remains a Python ACP stdio agent and remains authoritative for orchestration. A new public adapter and project read model are required; do not read internal SQLite from a client or parse Markdown to infer state.

The service brokers generic ACP capabilities separately from Cortex extensions. Explicit project activation is required. Use compact summaries for boards and bounded, paged history for detail. Client actions are authenticated, versioned commands; typed outcomes and authoritative snapshots are displayed. Existing Paseo integration is reference material, not a dependency or a migration prerequisite. Exact runtime workspace ownership and write isolation must be established against current source before implementing file mutation.

## Local desktop slice (built)

The first implementation milestone is a local vertical slice ([roadmap](roadmap.md), M2): a real Tauri window, the Rust machine service and the real Cortex runtime, carrying one owner journey from project registration to a runtime result. Its boundaries are:

| Location | Responsibility |
|---|---|
| `apps/cortex-workspace-desktop/` | Solid entry and Tauri 2 shell: native bridge with a closed operation allowlist, capabilities, local-content navigation policy and `.deb` configuration. |
| `crates/workspace-service/` | Tokio service: project locator registry, authenticated local socket, single-instance lock, child supervision, bounded project PTYs and the ACP client. It hosts Core task/history projections and a reusable draft writer under a cooperative project/channel lease; Python still owns execution. |
| `crates/workspace-protocol/` | Shared Rust envelope and DTO codecs for the shell and the service. No Tauri or WebKit dependency. |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | Exact Rust workspace and toolchain inputs. The default member is the headless protocol crate only; the service and the Tauri application are built and tested explicitly. |
| `crates/workspace-protocol/channels.json` | The dev and release channel identities, compiled into the shared crate. Dev is the default channel; the two channels use separate package, binary, unit, runtime, configuration and state names, plus `.cortex-dev` (Dev) and `.cortex` (release) project stores ([CW-S12](backlog/CW-S12.md)). |
| `packaging/container/`, `packaging/systemd/user/` | The container build for native targets and the `.deb` packager, and the foreground user-level units for each channel. Installing the package installs the units and per-user login-start symlinks. |
| `crates/cortex-xtask` (`scripts/cortex-xtask.sh build-deb`), `scripts/cortex-paseo-tools.sh clone-dev-settings` | The container build driver (dev by default; CI release on manual `main` runs; CI dev on manual `dev` runs) and the non-secret settings clone into the dev channel. |
| `packages/workspace-ui/` | Promoted accepted presentation: components, styles, router and theme preferences. |
| `packages/workspace-domain/` | Pure presentation invariants, worker activity types/tool-row projection and DTO-to-view-model mapping. It never executes lifecycle transitions or imports a host plugin. |
| `packages/workspace-contract/` | Generated TypeScript wire types, constants and shared valid and invalid fixtures. |
| `crates/workspace-protocol/schema/workspace.json` | Canonical versioned wire schema; the shared Rust types and the generated artifacts derive from it. |
| `crates/workspace-service`, `crates/cortex-core` | The native components that translate private runtime tables into public records: command ledger, projections, invalidation outbox and adapter-owned project metadata. |
| `scripts/cortex-xtask.sh generate-contract check|write` | Deterministic generation and check of the schema fingerprint, Rust and TypeScript types and shared fixtures. |

The service connects to the retained execution runtime through the public `_cortex/workspace` extension and generic ACP stdio connection. Task/history reads and draft creation (with or without attachments) now use its in-process Rust Core provider; native execution and engine-policy composition are not enabled. The local socket is the only service interface. The desktop shell never starts, stops or talks to an ACP child directly. Closing the window detaches the subscription and leaves the child running. The service runs as a user-level process from its complete bundled runtime. Package installation arranges login startup; the native desktop bridge starts its fixed channel unit when necessary. The webview never receives a general systemctl or process-spawn capability. The separate local-owner PTY surface exposes only open/output/input/resize/close and intentionally carries interactive shell authority ([security](security.md#owner-terminal-built-local-linux)). Local transport and the adapter decision are recorded in [ADR 0014](adr/0014-local-service-transport.md).

Workspace UI compiles only its own client code and framework-free shared packages, never `integrations/paseo`. `packages/workspace-ui/test/paseo-boundary.test.ts` checks the actual TypeScript program (including type-only imports and tests). Paseo retains self-contained copies of the pure presentation modules for its standalone plugin distribution; the same test requires byte-identical implementations, allowing only import-path/type-source adaptation for worker grouping. RPC schemas and Zod validation remain in the plugin, outside the Workspace compile graph.

Remote machines, relay and push are built after the local desktop ([roadmap](roadmap.md), M5). Remote machine registry and connector items stay separate from the local machine, which is the one machine the owner starts.

Remote-path infrastructure sits outside the desktop and phone applications. Two owner-operated container services run on one owner's server through Docker Compose: an `iroh-relay` fallback and a push gateway for metadata-only FCM signals. Clients install nothing extra and use built-in default URLs that the user can override ([ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md), [self-hosted services](self-hosted-services.md)).

[Protocol](protocol.md) · [remote](remote-access.md) · [concurrency](consistency-and-recovery.md).
