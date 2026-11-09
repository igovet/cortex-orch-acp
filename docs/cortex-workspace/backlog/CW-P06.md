# CW-P06: Self-hosted compose stack for iroh-relay and push-gateway: compose file, environment template without secrets, TLS, health checks, deploy, upgrade and rollback runbook

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Deliver one Docker Compose stack that runs the self-hosted `iroh-relay` and `push-gateway` services on the owner's server ([ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md)). The stack is reproducible from the repository, holds no secret values, and has tested health checks and a tested rollback. Production deployment is an owner-approved operation and is not performed by this item.

## Requirements
CW-R08 (reconnect without extra user-installed software), CW-R09 (privacy-preserving push with an explicit unavailable state) and CW-R16 (protect credentials and device identities; reject unauthorized use). See [requirements](../requirements.md).

## Inputs
- [Self-hosted services](../self-hosted-services.md): topology, verified and unverified facts, ports, TLS options, secrets, logs, backups, and update and rollback steps.
- [ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md) and [ADR 0010](../adr/0010-push-gateway-cloudflare.md) (superseded, history only).
- [CW-P01](CW-P01.md): push-gateway image, configuration contract, secret file path and health route.
- [CW-N05](CW-N05.md): relay configuration, access policy and logging policy.
- Owner-supplied inputs, none of which are in the repository: DNS name, public IP, exposed-port approval, relay access mode, and the server's operating system and Docker version.

## Deliverables
- `docker-compose.yml` with two services, `iroh-relay` and `push-gateway`. Images are pinned by version tag or digest. Each service has a health check and a restart policy. The file contains no secret values.
- `.env.example` containing variable names and placeholders only. A real `.env` and the secrets directory are excluded from git by ignore rules that this item adds and verifies.
- The relay configuration template (TOML) and its access-mode section, consistent with [CW-N05](CW-N05.md).
- TLS configuration for the selected option: relay-managed certificates, a reverse proxy, or manual certificates. The chosen option and its renewal procedure are recorded.
- A deployment, upgrade and rollback runbook. It covers prerequisites, DNS and firewall checks, secret placement, first start, health verification, pinning, rollback to the previous tag, log retention, backup and restore, and the owner approval step before any production change.
- A non-production validation procedure: a Compose project on a test host or a local Docker environment, with fixture secrets only.

## Acceptance
**Positive**
- On a non-production Compose project, both services start, pass their health checks, and restart cleanly after a container restart.
- The stack starts with a fixture FCM secret file and a fixture relay configuration. The push-gateway fixture test from CW-P01 passes inside the stack.
- A rehearsal of upgrade to a new tag and rollback to the previous tag succeeds, and health is verified after each step.
- The runbook has been followed end to end on the non-production project, and every step's outcome is recorded.

**Negative**
- An unauthenticated caller to the push gateway is rejected, and no FCM call is made.
- A payload that contains content fields is rejected, and nothing is forwarded.
- The FCM credential is absent from the repository, the image layers, the desktop build and the Android build. A scan for its private-key marker finds zero matches in each of these.
- Container logs collected from the stack contain no task content, payload body, full FCM token or secret value. A canary placed in a rejected payload does not appear in them.
- If relay access control is enabled (allowlist or shared token), an unlisted or unauthenticated endpoint is refused by the relay. If the owner has not chosen an access mode, the stack does not start with an undecided mode.
- The compose file and the environment template contain no secret values. A scan finds none.

## Dependencies
CW-P01 (push-gateway image and contract) and CW-N05 (relay configuration and access policy). The owner gate CW-D05 is recorded; Gate M does not gate this item. Production deployment requires owner authorization, the owner-provided DNS name and public IP, and the owner's server.

## Validation
- Static checks: `docker compose config` on the non-production project, plus the secret-value scan and the ignore-rule check.
- Runtime checks: health status for both services, a restart test, an upgrade and rollback rehearsal, and a log-redaction scan of collected container logs.
- Certificate checks: issuance or renewal in a staging or test setup, if the ACME option is chosen. Not run against production.
- Live FCM send to the owner's test device and any production deployment only after explicit owner authorization. Not run here.
- No check has been run for this planned item.

## Documentation impact
- [Self-hosted services](../self-hosted-services.md): replace each unverified fact with the verified value and its evidence.
- [Remote access](../remote-access.md) and [security](../security.md): deployment and trust boundary.
- [Risks and open decisions](../risks-and-open-decisions.md): owner server inputs and access-mode decision.
- [ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md): record any change in the packaging or TLS decision.
