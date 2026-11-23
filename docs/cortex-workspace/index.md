# Cortex Workspace

**Status: the Ubuntu desktop application is implemented and checked by no-paid automated checks and container journeys. It is not verified on a real Ubuntu session, with paid model runs or as a published release. Remote access (an embedded iroh host, pairing, an allowlisted tunnel, a Settings, Remote access page), the Android app and the relay stack in `deploy/relay/` are built and checked only by no-paid automated tests; they are not verified on a physical device or through a relay. Push and the desktop's own remote machines are planned and not implemented. Each requirement and backlog item states its own status.**

This bundle describes a standalone task-first ACP workspace: the desktop application is built, remote access and the Android app are built but not qualified, and push is a plan. No release is published. Start with [product](PRODUCT.md), [requirements](requirements.md), [architecture](architecture.md), [roadmap](roadmap.md) and [backlog](backlog/index.md).

- Signed desktop updates, maintenance, engine onboarding and the command map: [desktop controls](maintenance-and-controls.md).
- Current local behavior and limits: [task execution](local-task-execution.md).
- Design: [direction](DESIGN.md), [screens](SCREENS.md), [desktop concepts](design/desktop-concepts.md), [mobile concepts](design/mobile-concepts.md).
- Contracts: [domain](domain-model.md), [protocol](protocol.md), [recovery](consistency-and-recovery.md), [security](security.md), [remote and push](remote-access.md), [self-hosted services](self-hosted-services.md), [IDE and Git](ide-and-git.md).
- Delivery: [platforms](platforms.md), [performance](performance.md), [migration](migration-and-rollback.md), [testing](testing-and-acceptance.md), [risks](risks-and-open-decisions.md), [decisions](adr/index.md).

Desktop Gate D has passed. The local vertical slice ([roadmap](roadmap.md), M2) and the desktop screens on top of it are built with no-paid and container checks and are not a shipped release. The real conversation, durable queue, worker safe-point guidance and stop controls are described in the [local protocol](protocol.md#local-desktop-slice-built) and [screen inventory](SCREENS.md#local-slice-built-real-mode). Mobile follows the desktop: the owner approved the static phone mockups on 2026-11-17 ([Gate M](design/mobile-approval.md)), and Gate M gates only mobile and Android work.
