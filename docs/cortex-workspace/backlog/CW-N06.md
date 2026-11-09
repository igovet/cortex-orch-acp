# CW-N06: LAN fast path: desktop mDNS advertise + Android NsdManager discovery feeding direct-address dial, bypassing relay; same-Wi-Fi latency measurement

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
When the desktop and the phone share a network, dial the paired desktop NodeId directly using addresses from local discovery. This bypasses the relay without weakening authorization.

## Requirements
CW-R08 (secure reconnect) and CW-R16 (reject unauthorized devices). See [requirements](../requirements.md).

## Inputs
- [Remote access](../remote-access.md): local discovery when available, with direct connection preferred and relay fallback.
- The iroh opt-in mDNS local discovery on desktop. The Android side uses `NsdManager` because the desktop mDNS path is not used on mobile.
- CW-N01, CW-N02 and CW-N04 (allow-list enforcement applies to LAN connections too).

## Deliverables
- Desktop: opt-in mDNS advertisement carrying NodeId and direct port only. No project data and no device list is advertised.
- Android: `NsdManager` browse, filtered to the paired NodeId. The resolved address is passed to the iroh dial.
- Fallback: if the LAN dial fails, the connection moves to the relay path without user action.
- A same-Wi-Fi latency measurement procedure. It records median and p95 setup time and RPC round trip, against the relay path on the same hardware.

## Acceptance
**Positive**
- A paired phone on the same Wi-Fi connects direct. The endpoint path status shows that the relay was not used. Measured setup and round-trip values are recorded.
- With LAN discovery disabled, the connection uses the relay path automatically.

**Negative**
- An advertisement from an unpaired or unknown NodeId is ignored. No dial goes to an unknown address.
- A spoofed NSD record carrying the wrong NodeId fails the handshake. The phone does not accept the connection.
- An advertisement from a revoked device is not used (CW-N04).
- A LAN connection is still subject to the CW-N04 accept checks.
- Reject revoked or expired credentials: covered by CW-N04 for LAN connections.

## Dependencies
CW-N01, CW-N02, CW-N04. Owner gates CW-D05 and CW-M03 are mandatory and are satisfied transitively.

## Validation
- Physical Android on the same Wi-Fi as the Ubuntu desktop. Captured mDNS and NSD traffic. Latency samples recorded with device model, Android version and desktop hardware.
- Unit tests with NSD fixtures are fixtures only.
- No check has been run for this planned item.

## Documentation impact
- [Remote access](../remote-access.md): the LAN path and its relay fallback.
- [Performance targets](../performance.md): the latency measurement method used for the same-Wi-Fi case.
