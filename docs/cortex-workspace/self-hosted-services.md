# Self-hosted services

**Status: planned; no deployment, image, certificate, DNS record or port has been verified on a real server.**

Two services run on one server operated by the owner, through Docker Compose: the iroh relay (fallback transport) and the push gateway (metadata-only wake-up signals). This page is the planning reference. [CW-P06](backlog/CW-P06.md) turns it into a compose file, an environment template and a runbook. The decision is recorded in [ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md), which supersedes [ADR 0010](adr/0010-push-gateway-cloudflare.md).

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

## Verified and unverified facts

Verified from the iroh-relay documentation fetched on 2026-11-04 (iroh repository, `iroh-relay` directory). The documented version is not pinned, so re-check before deployment:

- Dev-mode plain HTTP port `3340`.
- QUIC address discovery is enabled with `enable_quic_addr_discovery = true`. The dev example uses port `7824`.
- Config is a TOML file passed as `--config-path`. Manual TLS uses `[tls]` with `cert_mode = "Manual"`, `manual_cert_path` and `manual_key_path`.
- Access modes: `access = "everyone"` (default), `access.allowlist` or `access.denylist` by endpoint ID, a `shared_token` (environment override `IROH_RELAY_ACCESS_TOKEN`), and an HTTP callout (`access.http.url`) that must return `true` to admit a connection.
- Server build uses the `server` feature: `cargo build --profile optimized-release --package iroh-relay --features server`.

Not stated in the fetched documentation, so unverified:

- The production HTTPS port and relay endpoint path.
- Whether a STUN service is exposed, and on which port.
- The production port and protocol of QUIC address discovery. `7824` is only the dev example.
- ACME or Let's Encrypt support, and whether the relay can sit behind a reverse proxy.
- Whether an official Docker image exists.
- A health endpoint.

## Ports and network

Open only the ports that verification shows are needed. Nothing else should listen on all interfaces.

| Port | Protocol | Purpose | Status |
|---|---|---|---|
| 443 | TCP | HTTPS for the relay and the push-gateway API | Planning assumption; production port not stated in the fetched docs |
| 80 | TCP | ACME HTTP-01 challenge, only if certificates are issued that way | To verify; ACME not stated in the fetched docs |
| 7824 | UDP to verify | QUIC address discovery | Dev-mode example only; production port and protocol to verify |
| STUN port | UDP | STUN, if the relay exposes it | Not stated; to verify before opening anything |

The push gateway should not be reachable without TLS.

## DNS and TLS

- One DNS name for the server, or one name per service. The owner provides the name and the public IP ([risks](risks-and-open-decisions.md)). Placeholders in this page: `<relay-domain>` and `<push-domain>`.
- Certificate option A: the relay obtains certificates itself. Unverified; ACME is not stated in the fetched docs.
- Certificate option B: a reverse proxy terminates TLS and forwards to the services. Unverified whether the iroh relay protocol works behind a proxy.
- Manual certificates are documented (`cert_mode = "Manual"`). The owner would then renew them.
- The push gateway's TLS is provided by a reverse proxy or the gateway itself. CW-P06 chooses one and records it.

## Compose layout (planned)

```text
docker-compose.yml     (in git; no values)
  services:
    iroh-relay         pinned image tag; config file mounted read-only; certificate volume;
                       health check; restart policy
    push-gateway       pinned image tag; FCM credential as a secret file; non-secret settings
                       from environment; health check; restart policy
  secrets:
    fcm_service_account   file outside git (for example ./secrets/fcm-service-account.json)
.env.example           (in git) variable names and placeholders only
.env                   (not in git) domain names, image tags, non-secret values
```

Docker Compose secret and health-check syntax must be checked against the installed Compose version before use.

## Secrets

- **FCM service-account JSON.** A file on the server, mounted as a Docker secret. Readable only by the service user. Never committed, never copied into an image, never placed in any client build.
- **Caller-authentication keys.** Their form and storage depend on the open caller-authentication decision ([CW-P01](backlog/CW-P01.md)). If registered installation keys are stored on the server, they go in a volume that holds no FCM tokens and no content.
- **Relay access token.** If `shared_token` access is chosen, the token comes from a file or environment variable that is not committed.
- Documents record only secret names, never values.

## Health checks

- `push-gateway`: an unauthenticated liveness route that returns no task, token or key data. The route is defined in [CW-P01](backlog/CW-P01.md).
- `iroh-relay`: no health endpoint is stated in the fetched docs. Verify one before use. If none exists, use a TCP connect check on the HTTPS port.

## Logs

- Logs contain no task content, no payload body, no full FCM token, no key material and no secret value.
- The push gateway logs `event_type`, a status and a token fingerprint ([CW-P01](backlog/CW-P01.md)).
- Relay logs contain connection metadata only, with bounded retention. The retention limit is recorded in the runbook ([CW-N05](backlog/CW-N05.md)).
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

Image provenance for `iroh-relay` is unverified, because no official image is stated in the fetched docs. Use either a build from pinned upstream source or a published image whose provenance has been verified.

## Related

[ADR 0011](adr/0011-self-hosted-relay-and-push-gateway.md) · [remote access](remote-access.md) · [security](security.md) · [risks](risks-and-open-decisions.md) · [CW-N05](backlog/CW-N05.md) · [CW-P01](backlog/CW-P01.md) · [CW-P06](backlog/CW-P06.md)
