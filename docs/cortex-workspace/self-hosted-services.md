# Self-hosted services

**Status: the relay stack exists in `deploy/relay/` (Compose file, `relay.toml`, `.env.example`, runbook); the push gateway is not built and has no stack. No deployment, certificate, DNS record or connection through the relay has been verified from this repository, and the relay has not been tested with the apps.**

Two services run on one server operated by the owner, through Docker Compose: the iroh relay (fallback transport) and the push gateway (metadata-only wake-up signals). The relay half is built as `deploy/relay/` (its `README.md` is the runbook and the source of truth for relay facts); the rest of this page remains the plan, in particular for the push gateway ([CW-P06](backlog/CW-P06.md)). The decision is recorded in [ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md), which supersedes [ADR 0010](adr/0010-push-gateway-cloudflare.md).

## Topology

```text
Desktop (Ubuntu) ──── direct QUIC (preferred) ──── Phone (Android)
      │                                                │
      └──── iroh-relay (fallback, ciphertext only) ────┘
      │
      └──── push-gateway (HTTPS, signed metadata) ──── FCM (external)
                 owner's server, Docker Compose
```

The relay has no external service dependency. The push gateway depends only on FCM. Clients install nothing extra.

## Services

| Service | Role | Holds | Must not hold |
|---|---|---|---|
| `iroh-relay` | Forwards ciphertext when a direct connection fails. Optional QUIC address discovery. | TLS material if the relay manages certificates; access-control configuration; optional access token | Project plaintext; device grants; task content |
| `push-gateway` | Authenticates desktop callers, rate-limits them and forwards closed metadata to FCM. | FCM sender credential (secret file); installation verification keys if stored (open, see [CW-P01](backlog/CW-P01.md)) | Task content; payload bodies; persisted FCM tokens |

## Relay facts (`deploy/relay/`)

The relay is the official `n0computer/iroh-relay` image, pinned to tag `v1.4.1`, matching the `iroh` 1.4.1 crate in the desktop service and the phone app (upgrade within 1.4.x). Facts below come from `deploy/relay/README.md` and `relay.toml`:

- It runs as plain HTTP on port 8080 inside the container (non-root, read-only, all capabilities dropped) behind an existing Traefik that terminates TLS and forwards WebSocket upgrades without extra configuration. No port is published.
- The client path is `/relay`. Health: `/healthz` returns `200` with a JSON status and version; `/ping` returns 200 and `/generate_204` returns 204. The Compose health check calls `/healthz` on the internal port.
- QUIC address discovery is **off** (`enable_quic_addr_discovery = false`). It needs its own UDP port (7842) and a certificate held by the relay, because Traefik does not carry that UDP traffic. Without it, clients still connect through the relay but get less help learning their public address, so direct paths may succeed less often and more traffic may stay on the relay.
- Access mode is `access = "everyone"`. An endpoint-id allowlist or denylist, a shared token and an HTTP callout are alternatives in `relay.toml` comments. Device authorization never rests on the relay.
- Metrics are off; if enabled, keep the metrics port (9090) off Traefik.
- The relay keeps no state, so upgrade and rollback only change `RELAY_IMAGE_TAG` in `.env` and run `docker compose pull && docker compose up -d`. Logs rotate at 3 x 10 MB; verbosity follows `RELAY_LOG`.
- What the relay sees: which endpoint identities connect, when, how much data moves and client IP addresses. It cannot read content or impersonate a device ([security](security.md#remote-access-built-not-qualified)).

Observed on 2026-11-18 against the deployed default relay (Cloudflare proxy with an Origin certificate at Traefik): `/healthz` answers 200 with version 1.4.1, `/ping` answers 200, and the live check `crates/workspace-remote/tests/relay.rs` paired a client that had no direct address and exchanged one request. Not verified: the desktop application and a phone connecting through it from different networks, long-lived sessions through the proxy, forced-relay data transfer, and operating cost.

## Ports and network

| Port | Protocol | Purpose | Status |
|---|---|---|---|
| 443 | TCP | HTTPS at Traefik for the relay (and later the push-gateway API) | Terminated by the existing Traefik; the relay itself listens on 8080 internally |
| 7842 | UDP | QUIC address discovery | Off by default; optional, see `deploy/relay/README.md` |
| 9090 | TCP | Relay metrics | Off; never expose it |

The push gateway should not be reachable without TLS. Its port is not defined because it is not built.

## DNS and TLS

- One DNS name for the relay (`A`, and `AAAA` if the server has IPv6). Two certificate routes are documented: a Cloudflare-proxied record with a Cloudflare Origin certificate already held by Traefik (zone SSL/TLS mode Full (strict), WebSockets on), where clients see Cloudflare's certificate; or a DNS-only record with a certificate issued by Traefik through a `certresolver` label. A DNS-only record with an Origin certificate fails the apps' certificate checks.
- The relay URL the apps use is the exact `https://<relay-domain>` form, with no path and no trailing slash. It is the value in `crates/workspace-remote/default-relay.txt` (the built-in default is `https://cortex-relay.vetrov.dev`) or a per-machine override on the desktop; the phone takes the relay from the pairing link.
- The push gateway's TLS is provided by a reverse proxy or the gateway itself. CW-P06 chooses one and records it.

## Compose layout

`deploy/relay/` holds `docker-compose.yml` (no values), `relay.toml`, `.env.example` (variable names and placeholders only) and `README.md`. `.env` (the relay host name, image tag and Traefik names) is git-ignored; the relay needs no secret. The push gateway service and its `fcm_service_account` secret file are planned and not in the stack.

Docker Compose secret and health-check syntax must be checked against the installed Compose version before use.

## Secrets

- **FCM service-account JSON.** A file on the server, mounted as a Docker secret. Readable only by the service user. Never committed, never copied into an image, never placed in any client build.
- **Caller-authentication keys.** Their form and storage depend on the open caller-authentication decision ([CW-P01](backlog/CW-P01.md)). If registered installation keys are stored on the server, they go in a volume that holds no FCM tokens and no content.
- **Relay access token.** None is used (`access = "everyone"`). If `shared_token` access is chosen, the token comes from a file or environment variable that is not committed.
- Documents record only secret names, never values.

## Health checks

- `push-gateway`: an unauthenticated liveness route that returns no task, token or key data. The route is defined in [CW-P01](backlog/CW-P01.md).
- `iroh-relay`: `/healthz` (see above), checked by the Compose health check.

## Logs

- Logs contain no task content, no payload body, no full FCM token, no key material and no secret value.
- The push gateway logs `event_type`, a status and a token fingerprint ([CW-P01](backlog/CW-P01.md)).
- Relay logs contain connection metadata only, with bounded retention (3 x 10 MB in the Compose file).
- Container log rotation (maximum size and file count for the logging driver) bounds disk use. Check the option names against the installed Docker version.
- Logs are never copied into repository documents or reports.

## Backups and monitoring

- Owner responsibility. Back up the relay configuration, certificate material if it cannot be re-issued, and the gateway's registration store if one exists.
- The FCM credential is kept in an encrypted or otherwise owner-controlled offline copy, never in an unencrypted repository or shared storage.
- Restore must be rehearsed. A backup that has not been restored is not a backup claim.
- Monitoring: an uptime check on the HTTPS endpoint is the minimum. The owner chooses the tool. Open.

## Update and rollback

1. Pin each image by version tag or digest in `.env`. Record the previous tag before changing it.
2. Rehearse the change on a non-production Compose project first.
3. Pull the new images and run `docker compose up -d`. Check health status.
4. To roll back, set the previous tag and run `docker compose up -d` again. Check health status.
5. Record the version, date and outcome of each change. Do not record secrets.

The relay image is the official `n0computer/iroh-relay` image at a pinned tag; its digest is not pinned here.

## Related

[ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md) · [remote access](remote-access.md) · [security](security.md) · [risks](risks-and-open-decisions.md) · [CW-N05](backlog/CW-N05.md) · [CW-P01](backlog/CW-P01.md) · [CW-P06](backlog/CW-P06.md)
