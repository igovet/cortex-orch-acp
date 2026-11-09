# Embedded remote access

**Status: planned; no implementation or qualification is implied.**

Decision: Plan embedded iroh direct and relay connectivity and QR enrollment, with a zero-knowledge iroh relay that forwards only ciphertext and serves only as a fallback. Relay hosting is one self-hosted `iroh-relay` on the owner's server, as decided in [ADR 0011](0011-self-hosted-relay-and-push-gateway.md) ([CW-N05](../backlog/CW-N05.md)); the n0 public relay is not the default. Tailscale or another separately installed VPN is rejected as a prerequisite. A public n0 relay is off by default and needs an owner trust decision. The push gateway is a separate self-hosted container and is not part of the relay ([ADR 0011](0011-self-hosted-relay-and-push-gateway.md)). Relay operations and security remain to be validated.

Consequences: implementation and qualification are pending; revisit if feasibility or security validation fails.
