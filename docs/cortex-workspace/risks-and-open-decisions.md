# Risks and open decisions

**Status: planned; no implementation or qualification is implied.**

- Android stack is provisionally React Native New Architecture; the physical-device comparison with Tauri Android ([CW-F02a](backlog/CW-F02a.md), [CW-F02b](backlog/CW-F02b.md), decision record [CW-F02](backlog/CW-F02.md)) is a go/no-go decision before Android production work.
- iroh Android bindings, LAN discovery, relay privacy, availability, operating cost and forced-relay tests remain unverified. The n0 public relay stays disabled by default; enabling it requires an explicit trust/operations decision.
- FCM requires provider infrastructure and supported services; no delivery SLA or no-GMS support is assumed.
- Rust service/public adapter and actual Cortex workspace isolation do not exist as specified; verify before lease design.
- WebKitGTK GPU behavior, editor/LSP compatibility and performance budgets need real hardware measurement.
- Git hooks, signing, credentials, race handling and cross-device writes require executable qualification.
- Exact design tokens and approval records are pending. Release distribution, upgrades and disaster recovery need owner decisions before operation.
- Push gateway caller authentication is not defined yet. [CW-P01](backlog/CW-P01.md) proposes a per-installation key registered at enrollment, with signed requests; the owner must decide before implementation. No shared secret may be embedded in client builds. Where registered installation keys are stored on the owner's server is part of the same decision.
- Push task reference: [CW-P01](backlog/CW-P01.md) requires a random or keyed opaque reference, not a plain hash of a guessable identifier. An earlier draft used a plain hash; this deliberate deviation needs owner review.
- Public relay fallback: the n0 public relay stays disabled by default; enabling it requires a recorded owner trust decision ([CW-N05](backlog/CW-N05.md)).
- Relay access control: the owner must decide whether the relay accepts only the owner's paired devices (an allowlist of endpoint IDs), accepts any endpoint, or uses a shared token. The fetched iroh-relay documentation lists these modes, but the production configuration is unverified. The choice is implemented and tested in [CW-N05](backlog/CW-N05.md). Device authorization is enforced at the desktop either way ([CW-N04](backlog/CW-N04.md)).
- Owner server inputs: the owner must provide the server's domain name, its public IP address and the ports to open. Exact ports, certificate mode and the verification steps are in [self-hosted services](self-hosted-services.md). Required before deployment ([CW-P06](backlog/CW-P06.md)).
- Owner server operations: backups, monitoring, log retention limits and the restore rehearsal are owner responsibilities. Choices are open until the owner records them ([self-hosted services](self-hosted-services.md)).
- Operating cost of the owner's server is an estimate. It is not verified.
