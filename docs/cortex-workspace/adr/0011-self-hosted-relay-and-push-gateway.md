# Self-hosted relay and push gateway via Docker Compose

**Status: planned; supersedes [ADR 0010](0010-push-gateway-cloudflare.md); no implementation or qualification is implied.**

## Context

The Android release, which follows the Ubuntu desktop first release, sends FCM wake-up signals to Android. Android does not keep background sockets reliably alive, and the FCM sender credential cannot be embedded in the desktop app or any client build ([ADR 0009](0009-push.md)). Users install nothing beyond the product ([product](../PRODUCT.md)). Pairing and data transport use embedded iroh ([ADR 0004](0004-embedded-remote.md)), and an iroh relay is the fallback when a direct connection fails.

[ADR 0010](0010-push-gateway-cloudflare.md) placed the push gateway on Cloudflare Workers. The owner has now made a binding decision that replaces that placement: both server-side components run on the owner's own server through Docker Compose. Neither runs on Cloudflare, and no Cloudflare account is required.

## Decision

- Both server-side components run on one server operated by the owner, as two services of one `docker-compose.yml`:
  - `iroh-relay`: the upstream `iroh-relay` server. It is a fallback only; direct peer-to-peer connection between phone and desktop stays preferred ([CW-N05](../backlog/CW-N05.md)).
  - `push-gateway`: a stateless forwarder that holds the FCM sender credential, authenticates desktop callers, rate-limits them and forwards metadata-only wake-up signals. It never receives task content ([CW-P01](../backlog/CW-P01.md)).
- The compose stack, TLS, health checks, secret handling and update and rollback procedure are described in [self-hosted services](../self-hosted-services.md) and delivered by [CW-P06](../backlog/CW-P06.md).
- Worker-style gateway code may optionally be self-hosted with Cloudflare's open-source `workerd` runtime. This is an option, not a requirement.
- Secrets (the FCM service-account JSON and caller-authentication keys) stay outside git. Containers receive them as Docker secrets or from an env file that is not committed.
- End users install nothing extra. The relay URL and push gateway URL are part of the product configuration. The built-in default points at the owner's server, and the user can override it.
- The n0 public relay is not the default. It may remain an explicitly disabled, owner-gated optional fallback that requires a recorded owner decision ([CW-N05](../backlog/CW-N05.md)).

## Rejected alternatives

- **Cloudflare Workers gateway ([ADR 0010](0010-push-gateway-cloudflare.md)).** Requires a Cloudflare account, a Workers plan and free-tier limits that the owner does not want the push path to depend on. Superseded by this record.
- **Cloudflare Tunnel as transport.** A persistent tunnel needs a hostname tied to a Cloudflare account and domain, terminates TLS at Cloudflare, and does not solve push. Still rejected.
- **n0 public relays as the default.** The project would depend on infrastructure it does not control, under a trust and availability model the owner has not accepted. They remain only as the disabled optional fallback above.
- **A VPS per user.** Each user would have to run and maintain servers, which contradicts the requirement that nothing is installed separately.
- **Embedding the FCM credential in clients.** Every installation would hold a sender credential able to push to arbitrary FCM tokens.

## Consequences

- The owner operates, updates, monitors and backs up the server. Uptime, backups, certificate renewal and credential rotation are owner responsibilities. No project-run hosted service exists.
- The server is a single point of failure for the relay fallback and for push. Direct connections and opening the app still work without push. A connection that cannot go direct depends on the relay and is unavailable while the relay is down.
- The FCM credential lives on the owner's server. Compromise of that server exposes it, so the threat model must cover it ([security](../security.md)).
- Both services are reachable from the internet. Their DNS name, public IP, exposed ports and relay access control are owner decisions listed in [risks and open decisions](../risks-and-open-decisions.md).
- The Cloudflare account, Workers plan and Cloudflare free-tier limits are no longer decision items.
- Nothing has been deployed or qualified. Exact iroh-relay ports, certificate mode and container packaging are verified only where [self-hosted services](../self-hosted-services.md) states it. Operating cost is an owner estimate and is not verified.
