# Cortex Workspace

**Status: in development, not shipped. Planned surfaces and built local behavior are distinguished below and in the linked contracts.**

This bundle describes a standalone task-first ACP workspace in development, not a shipped feature. Start with [product](PRODUCT.md), [requirements](requirements.md), [architecture](architecture.md), [roadmap](roadmap.md) and [backlog](backlog/index.md).

- Maintenance, engine onboarding and the command map: [desktop controls](maintenance-and-controls.md).
- Current local behavior and limits: [task execution](local-task-execution.md).
- Design: [direction](DESIGN.md), [screens](SCREENS.md), [desktop concepts](design/desktop-concepts.md), [mobile concepts](design/mobile-concepts.md).
- Contracts: [domain](domain-model.md), [protocol](protocol.md), [recovery](consistency-and-recovery.md), [security](security.md), [remote and push](remote-access.md), [self-hosted services](self-hosted-services.md), [IDE and Git](ide-and-git.md).
- Delivery: [platforms](platforms.md), [performance](performance.md), [migration](migration-and-rollback.md), [testing](testing-and-acceptance.md), [risks](risks-and-open-decisions.md), [decisions](adr/index.md).

Desktop Gate D has passed. The local vertical slice ([roadmap](roadmap.md), M2) is built with its no-paid and container checks and is not a shipped release. The real conversation, durable queue, worker safe-point guidance and stop controls are described in the [local protocol](protocol.md#local-desktop-slice-built) and [screen inventory](SCREENS.md#local-slice-built-real-mode). Mobile is deferred: Gate M gates only mobile and Android work.
