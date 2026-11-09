# CW-P01: Push gateway as a self-hosted container (stateless forwarder; FCM credential as a Docker secret); metadata-only payload schema enforced (no plaintext title/body)

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Provide a stateless push gateway that runs as a container service on the owner's server ([ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md)). It accepts only the closed metadata schema from paired desktops, authenticates the calling desktop, and forwards an FCM high-priority data message. The FCM sender credential is available only as a Docker secret file on the owner's server. The gateway is not deployed to Cloudflare, and no Cloudflare account is needed.

## Requirements
CW-R09 (privacy-preserving Android push with an explicit unavailable state) and CW-R16 (protect credentials; no content leaves the desktop in push traffic). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): FCM through a push gateway; messages carry only minimal opaque identifiers.
- [Security design](../security.md): push is an opaque notification hint, never an authority to execute commands.
- [ADR 0009](../adr/0009-push.md) and [ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md) (which supersedes [ADR 0010](../adr/0010-push-gateway-cloudflare.md)): placement on the owner's server; Cloudflare Tunnel rejected as transport.
- [Self-hosted services](../self-hosted-services.md): ports, TLS options, secrets, logs, health checks and rollback.
- CW-C02 (outbox event types that the desktop maps to the schema).
- Open design gap: the design does not define how the gateway authenticates desktop callers. Without this, any caller could use the gateway to push to arbitrary FCM tokens. The proposed default is a per-installation key registered during enrollment (CW-N03), with signed requests. It also does not define where registered installation keys are stored on the server, if they are stored. Both points need an owner decision before implementation. No shared secret may be embedded in client builds.

## Deliverables
- A Firebase project. The FCM sender credential is stored only as a Docker secret file on the owner's server. It is never committed to the repository, never copied into an image or the gateway source, and never present in any client build.
- Gateway source, tests and a container image build definition (Dockerfile) in the repository. The image contains no credential and no owner-specific value. The implementation language and runtime are chosen in this item and recorded. Options include a small container service, or Worker-style code run by Cloudflare's open-source `workerd` runtime.
- The image's configuration contract: environment variable names, the secret file path, the health route and the listening port. The compose service entry is delivered by [CW-P06](CW-P06.md).
- An HTTPS endpoint that accepts this closed schema: `event_type` from a fixed enum (`decision_requested`, `question_asked`, `result_ready`); `task_ref` as a non-guessable random event-scoped reference, generated once and persisted with the outbox event before dispatch. The same reference survives retries and restart, and distinct events on the same task receive distinct references (never a title, path or content hash). Also `ts` and `fcm_token`. Unknown fields are rejected. The desktop resolves the reference to the event and task only over the authenticated channel.
- Caller authentication per the open decision above, with per-installation rate limits. Rate-limit state holds only installation identifiers and counters, with a bounded TTL, in memory. If any registered-key store is persisted, it holds only installation identifiers and verification keys, never tokens or content.
- No token or payload storage. Retry state is in memory with a bounded TTL. Logs record `event_type`, a status and a token fingerprint, never the full token, payload body or task content.
- Forwarding as an `android.priority = high` data message: use `fcm_token` only as the recipient routing token, separate from the application data. The data payload contains exactly the validated `event_type`, `task_ref` and `ts` fields. The gateway fails closed: if the secret file is missing or unreadable, it makes no FCM call.
- Gateway-specific deployment steps: credential rotation by replacing the secret file and restarting the service, and rollback to the prior image tag. These are rehearsed on a non-production Compose project. The shared upgrade procedure is in [self-hosted services](../self-hosted-services.md).

## Acceptance
**Positive**
- A valid metadata message from an authenticated installation produces an FCM request with `fcm_token` as its routing token and exactly `event_type`, `task_ref` and `ts` in its data payload. This is verified by capturing the request in a fixture FCM endpoint.
- After a load test, the gateway has written no token or payload to disk or to any volume. This is checked by inspecting the container's volumes and its test logs.
- A rehearsal of deploy, secret rotation and rollback to the prior image tag succeeds on a non-production Compose project.

**Negative**
- A payload containing `title`, `body`, `path`, `prompt`, a content field or any unknown key is rejected with HTTP 400, and nothing is forwarded.
- A request with a missing, wrong or unregistered installation signature is rejected as unauthenticated and makes no FCM call.
- An oversized payload (bound to 4 KiB) is rejected.
- An FCM `UNREGISTERED` response returns an explicit `token_invalid` to the caller. The gateway does not retry in a loop.
- Logs and error bodies contain no full FCM token.
- Gateway logs contain no task content, payload body or full token. A canary string placed in a rejected payload's content field does not appear in the collected logs.
- The FCM credential is absent from the repository, the gateway source, the image and the desktop and Android build outputs. A scan for its private-key marker finds zero matches in each.
- A missing or unreadable secret file makes the gateway fail closed: no FCM call is made and the error body reveals no secret.
- Reject revoked or expired credentials: an installation revoked at the desktop (CW-N04) is rejected by the gateway once its registration is withdrawn.

## Dependencies
CW-C02. The owner gate CW-D05 is recorded; Gate M does not gate this item. Installation-key registration is validated end to end in the desktop dispatch item once enrollment exists; until then, gateway tests use fixture keys.

## Validation
- Gateway tests with a fixture FCM endpoint, covering schema validation, authentication, size limits and fail-closed secret handling. The test runner is chosen in this item.
- Log-redaction test: collect gateway logs from schema-valid and rejected runs and scan for the canary and the full token.
- Container build and run check, followed by the deploy, rollback and secret-rotation rehearsal on a non-production Compose project.
- Live FCM send to the owner's test device, and any production deployment, only after explicit owner authorization. Not run here.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): gateway trust boundary and schema.
- [Security design](../security.md): gateway authentication and the credential boundary.
- [Self-hosted services](../self-hosted-services.md): record the verified health route, port and secret handling.
- [ADR 0009](../adr/0009-push.md) and [ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md): record the caller-authentication decision once made.
