# CW-N03: QR/link enrollment: desktop NodeId + relay URLs + one-use secret; phone dial + local desktop confirmation; replay/expiry rejection tests

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Let a phone enroll with a desktop by scanning a time-limited QR code or opening an equivalent link. The payload carries the desktop NodeId, relay URLs and a one-use secret. The desktop must confirm locally before any trust is added.

## Requirements
CW-R08 (secure enrollment and reconnect) and CW-R16 (protect device identities; reject unauthorized operations). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): QR enrollment with a short-lived one-use secret, then local desktop confirmation.
- [Security design](../security.md): expiring single-use enrollment secret; explicit local confirmation; device key bound to a limited grant.
- CW-N01 (desktop NodeId and endpoint) and CW-N02 (phone endpoint bridge).

## Deliverables
- Desktop enrollment command. It produces a payload containing protocol version, desktop NodeId, relay URL(s), a random pairing secret of at least 128 bits and an expiry of about five minutes. The payload carries no engine credentials, Git credentials, project paths or task data.
- A link variant that carries the same payload. It must not be written to logs.
- Phone dial over a dedicated pairing ALPN, presenting the secret. The desktop verifies single use and expiry.
- A desktop confirmation dialog that shows the phone's label and the requested scope (read-only or full control, and which projects). Trust-on-first-use is not allowed. On confirm, the desktop writes an allow-list entry (handed to CW-N04).
- Atomic single use: the secret is consumed by the first valid presentation, and a declined confirmation also consumes it.

## Acceptance
**Positive**
- Scan → dial → confirm → phone shows the paired state. The allow-list entry carries the chosen scope.
- The desktop confirmation shows the device label and scope before the user confirms.

**Negative**
- A replayed secret is rejected, and the allow-list is unchanged.
- An expired secret is rejected with an explicit expired state. Expiry is tested with an injected clock and a real-time check.
- When two NodeIds present the same secret at the same time, exactly one wins.
- A declined confirmation creates no allow-list entry and consumes the secret.
- A malformed payload, an unsupported protocol version or an unknown relay URL scheme is rejected before any dial.
- The QR payload and secret are absent from logs and diagnostics.
- Reject revoked or expired credentials: an unpaired phone cannot run any RPC. That enforcement belongs to CW-N04.

## Dependencies
CW-N01, CW-N02. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Rust tests for the secret store: replay, expiry with an injected clock, and a concurrency race that must produce exactly one winner.
- Kotlin unit tests for payload parsing.
- Rendered confirmation dialog on Ubuntu, with a screenshot recorded.
- Physical Android dial over LAN and over a remote network. Before CW-N05 exists, the test relay is a non-production fixture.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): enrollment steps and the confirmation rule.
- [Security design](../security.md): pairing-secret lifetime and negative cases.
