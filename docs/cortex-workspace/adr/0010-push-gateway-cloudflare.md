# Push gateway on Cloudflare Workers; Cloudflare Tunnel rejected as transport

**Status: superseded by [ADR 0011](0011-self-hosted-relay-and-push-gateway.md); kept as history. No implementation or qualification is implied.**

This record is not current guidance. Its push gateway placement on Cloudflare Workers was replaced by the owner's binding decision to run both server-side components on the owner's own server through Docker Compose.

## Context

The Android release, which follows the Ubuntu desktop first release, sends FCM wake-up signals to Android. Android does not keep background sockets reliably alive, and the FCM sender credential cannot be embedded in the desktop app or in any client build. The earlier plan required two project-run servers: an iroh relay and a push sender. The owner rejected a plan that requires the project to keep its own servers for the push path, and nothing may be installed separately by the user. Pairing and data transport remain on embedded iroh ([ADR 0004](0004-embedded-remote.md)). Cloudflare Tunnel was proposed as an alternative transport.

## Decision

- **Push gateway:** a stateless forwarder that runs as a Cloudflare Worker. It holds the FCM credential as a Worker secret, authenticates desktop callers, rate-limits them, and accepts only the metadata-only schema ([CW-P01](../backlog/CW-P01.md)). For valid requests it transiently receives a recipient token and a closed metadata payload, never task content; it does not persist tokens or payload bodies. Requests containing content fields are rejected, and their bodies are not logged.
- **Transport:** embedded iroh for phone-to-desktop connectivity, direct where possible. The iroh relay is used only as a fallback when a direct connection fails; its hosting is an open owner decision ([CW-N05](../backlog/CW-N05.md)). `iroh-relay` is a native Rust server and cannot run on Cloudflare Workers. This is a planning assumption to confirm.
- **Cloudflare Tunnel** is rejected as the primary transport.

## Rejected alternatives

- **Cloudflare Tunnel as primary transport.** A persistent tunnel needs a Cloudflare account and domain per user, or a project-run provisioning service. Cloudflare terminates TLS, so end-to-end encryption and QR pairing would still be required on top. A tunnel does not solve push.
- **Quick tunnels.** Their hostnames are ephemeral, so they do not give a paired device a stable address. Terms and behavior were not verified in this decision.
- **VPS-hosted gateway.** Requires a machine the project must maintain, which contradicts the goal of running nothing separately for the push path.
- **Embedding the FCM credential in the desktop app.** Every installation would hold a sender credential able to push to arbitrary FCM tokens.
- **UnifiedPush as the default.** Requires the user to install an extra distributor app, and default users would still need FCM. It remains an opt-in post-v1 path ([CW-P05](../backlog/CW-P05.md)).

## Consequences

- The push path depends on Cloudflare. If the gateway is unavailable, pairing and data sync still work over iroh, but push does not, and the UI must show push as unavailable.
- Free-tier limits, checked on 2026-11-04 from Cloudflare's Workers limits page ([developers.cloudflare.com/workers/platform/limits](https://developers.cloudflare.com/workers/platform/limits/)): 100,000 requests per day; 10 ms CPU time per HTTP request; 50 subrequests per request; 64 MiB Worker size; 100 Workers per account; 64 secrets and environment variables per Worker, each up to 5 KB. These figures must be re-verified before deployment. Pricing was not verified and remains to be checked.
- Worker secrets are set with `wrangler secret put` or a versioned secret upload ([Cloudflare secrets documentation](https://developers.cloudflare.com/workers/configuration/secrets/)). Values are not visible in Wrangler or the dashboard after they are set. Rotation and rollback procedures belong to [CW-P01](../backlog/CW-P01.md). Only secret names may be recorded, never values.
- Cloudflare account ownership and the Workers plan are an open owner decision ([risks](../risks-and-open-decisions.md)).
- Nothing has been deployed or qualified. Android push delivery remains unverified.
