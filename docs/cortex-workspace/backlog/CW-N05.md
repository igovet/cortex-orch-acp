# CW-N05: Self-hosted iroh relay with RelayMap configuration and access policy; n0 public relay disabled by default as an owner-gated fallback; forced-relay integration test (hole-punch disabled) verifying ciphertext-only visibility

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Run the iroh relay as the self-hosted `iroh-relay` service on the owner's server ([ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md)). The relay is a fallback used only when a direct connection fails, and it forwards ciphertext only. The n0 public relay is not the default. It may remain an explicitly disabled optional fallback that the owner enables only with a recorded decision. The relay is never hosted on Cloudflare Workers, because `iroh-relay` is a native Rust server (planning assumption to confirm). A forced-relay test proves ciphertext-only visibility.

## Requirements
CW-R08 (reconnect without extra installed software) and CW-R16 (protect device identities; the relay sees no plaintext). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): the relay forwards encrypted traffic and must not receive project plaintext.
- [Self-hosted services](../self-hosted-services.md): verified and unverified iroh-relay facts, ports, TLS options and access modes.
- [Risks and open decisions](../risks-and-open-decisions.md): the relay access-control question, the public-fallback trust decision and the owner's server inputs.
- [Security design](../security.md): threat-model relay compromise before implementation.
- [ADR 0011](../adr/0011-self-hosted-relay-and-push-gateway.md): the push gateway is a separate container service and is not part of this relay.
- CW-N01 (endpoint and RelayMap configuration).

## Deliverables
- Relay configuration template (TOML) for the `iroh-relay` service, with no secret values. The template is consumed by the compose stack in [CW-P06](CW-P06.md).
- Access policy: the owner selects one mode, from `everyone`, an allowlist of paired endpoint IDs, or a shared token. The selected mode is recorded in [risks and open decisions](../risks-and-open-decisions.md). This item implements and tests the selected mode.
- RelayMap configuration. The relay URL defaults to the owner's server and can be overridden in the product. The n0 public relay is disabled by default. Enabling it requires an owner decision record, and configuration validation refuses to start without one.
- A forced-relay integration test with hole-punching disabled. It records relay-side logs and captures to show that only ciphertext and connection metadata are visible.
- A relay logging policy: no payload is logged, and metadata retention is bounded. The retention limit is recorded in the runbook.

## Acceptance
**Positive**
- In the forced-relay test, a paired desktop and a client harness exchange RPC through the configured relay. The Android leg is run in CW-Q02b.
- The default relay URL is used when no override is set, and the override is used when one is set.

**Negative**
- A canary string sent over the channel does not appear in relay logs, relay captures or any relay-side state.
- With the configured relay unreachable and fallback disabled, the connection reports `relay_unavailable`. It does not fall back silently to any other relay.
- Enabling the n0 fallback without an owner decision record makes configuration validation fail.
- With the allowlist or shared-token mode enabled, an endpoint that is not listed or does not present the token is refused by the relay. With `everyone`, the relay accepts any endpoint.
- The relay is not relied on for device authorization. A non-allow-listed NodeId that reaches the relay still cannot reach the desktop, because CW-N04 rejects it at accept.
- Reject revoked or expired credentials: device revocation is enforced at the desktop (CW-N04), not at the relay.

## Dependencies
CW-N01. The owner gate CW-D05 is recorded; Gate M does not gate this item. Deployment needs the owner's server, DNS name and approval, and is performed by CW-P06 only after owner authorization. The item does not depend on the push gateway.

## Validation
- Integration test (`cargo test`) with a locally built `iroh-relay` (server feature enabled) and hole-punching disabled. The test inspects relay-side logs and captures.
- Access-mode tests for the selected mode, plus the negative tests above.
- Relay funding, bandwidth and operating cost are owner estimates until the owner reviews them.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): relay hosting, trust model, access policy and fallback policy.
- [Security design](../security.md): relay threat model, including compromise and metadata visibility.
- [Risks and open decisions](../risks-and-open-decisions.md): access-mode decision and the public-fallback decision.
- [Self-hosted services](../self-hosted-services.md): record verified port and access-mode facts.
