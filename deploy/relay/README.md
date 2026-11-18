# Self-hosted relay

One `iroh-relay` 1.4.x server (the official `n0computer/iroh-relay` image, tag
`v1.4.1`, matching the `iroh` 1.4.1 crate embedded in the desktop service and
the phone app). It runs as plain HTTP on the internal Docker network and sits
behind an existing Traefik that terminates TLS.

## Prerequisites

- A Docker host with Compose v2 and a running Traefik whose Docker provider
  watches the external network named in `TRAEFIK_NETWORK` (default
  `traefik`), with an HTTPS entrypoint and a certificate resolver.
- A DNS name for the relay, for example `relay.example.com`.

## DNS

Create an `A` (and, if the server has IPv6, `AAAA`) record for the relay host
pointing at the server's public address.

Two certificate routes work; pick the one the server already uses for the zone:

- **Cloudflare proxy with an Origin certificate** (the compose default). Set the
  record to **Proxied**. Traefik must already hold a Cloudflare Origin
  certificate that covers the name, and the zone's SSL/TLS mode must be
  **Full (strict)**. WebSockets must be on for the zone (Network settings; on by
  default). Clients then see Cloudflare's public certificate. An Origin
  certificate is trusted only by Cloudflare, so a **DNS only** record with it
  fails certificate checks in the apps.
- **Direct, with a certificate issued by Traefik.** Set the record to **DNS
  only** and add the commented `certresolver` label in `docker-compose.yml`.
  Traefik skips issuing when a certificate in its own store already covers the
  name, for example a wildcard Origin certificate; use a name that certificate
  does not cover.

## Start

```sh
cd deploy/relay
cp .env.example .env      # edit RELAY_HOST and, if needed, the TRAEFIK_* names
docker compose up -d
docker compose ps         # wait for "healthy"
```

`.env` is git-ignored. No secret is needed. The relay config is `relay.toml`.

## Verify from outside

```sh
curl -sS -i https://relay.example.com/healthz
```

Expected: `HTTP/2 200` and a body like `{"status":"ok","version":"1.4.1",...}`.
Also `/ping` returns 200 with an empty body and `/generate_204` returns 204.
The client path is `/relay` (a WebSocket upgrade; a plain request returns
400 "missing header: upgrade", which shows the relay answers). Traefik
forwards WebSocket upgrades without extra configuration, and the relay needs
no `X-Forwarded-*` headers.

## Logs

```sh
docker compose logs -f iroh-relay
```

Verbosity follows `RELAY_LOG` (for example `info` or `iroh_relay=debug`).
Logs rotate at 3 x 10 MB.

## Upgrade and rollback

Set `RELAY_IMAGE_TAG` in `.env` to the wanted exact tag (use a 1.4.x tag so it
keeps matching the apps), then:

```sh
docker compose pull && docker compose up -d
```

To roll back, set the previous tag and run the same two commands. The relay
keeps no state, so nothing needs migrating or restoring.

## Access mode

`relay.toml` uses `access = "everyone"`: any endpoint may relay. Restricting
to an allowlist or denylist of endpoint ids is shown in comments there. Device
authorization never rests on the relay.

## What the relay can and cannot see

Traffic between devices is end-to-end encrypted. The relay forwards
ciphertext and sees which endpoint ids connect, when, how much data moves,
and the client IP addresses. It cannot read content, cannot impersonate a
device, and decides nothing about which devices may control which machine.
Once a direct connection is possible, traffic stops using the relay.

## QUIC address discovery (off)

It needs its own UDP port (7842) and a certificate that the relay itself holds,
because Traefik does not carry this UDP traffic. It is therefore off. Without
it, clients still connect through the relay but get less help learning their
public address for direct connections, so direct (hole-punched) paths may
succeed less often and more traffic may stay on the relay. To enable it, add a
`[tls]` section with `cert_mode = "Manual"` or `"Reloading"` and certificate
paths, set `enable_quic_addr_discovery = true`, mount the certificate, and
uncomment the UDP `ports` entry in `docker-compose.yml`.

## Metrics

Off by default (`enable_metrics = false`). If enabled, keep the metrics port
(9090) off Traefik and the public network.

## Value to report back

The relay URL, exactly in this form:

```
https://relay.example.com
```

(your real host, `https`, no path, no trailing slash).
