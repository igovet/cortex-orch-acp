# CW-Q02b: Phone fault and race rows

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q02](CW-Q02.md) for the phone and the device rows. This is a mobile qualification item and is gated by Gate M. The desktop rows are in [CW-Q02a](CW-Q02a.md).

## Goal
Add the phone and device rows to the fault and race matrix of [CW-Q02a](CW-Q02a.md), using the phone leg of the scenario in [CW-Q01b](CW-Q01b.md). Assert the documented invariants for devices: a revoked device is refused and its live sessions close, drift on the phone produces an explicit conflict, and a network handover does not replay prompts or decisions.

## Requirements
CW-R08 (secure enrollment and reconnect), CW-R12 (Android qualification) and CW-R14 (measured evidence). See [requirements](../requirements.md).

## Inputs
- [CW-Q02a](CW-Q02a.md): matrix format, seed and iteration record, and invariants.
- [CW-Q01b](CW-Q01b.md) (phone scenario), CW-I05 (Android small-file edit contract and drift), CW-G05b (Android Git screens, with no acceptance→push shortcut), CW-N04 (device allow-list and immediate revocation) and CW-N07 (network-change resilience).

## Deliverables
- Matrix rows, in the CW-Q02a format, for: a revoked device that reconnects; two devices (desktop and phone) submitting conflicting decisions at once; human and agent edits drifting apart on the phone (CW-I05); and a Wi-Fi to cellular handover at defined points.
- A Git row showing that an accepted result on the phone offers no direct push.
- An injection harness for network handover and revocation on the physical device.
- A seed and iteration count recorded for every race row.

## Acceptance
**Positive**
- Each row runs the recorded number of iterations and its invariant holds in every iteration.

**Negative**
- A revoked device is refused, and its live sessions are closed.
- Drift on the phone produces an explicit conflict. The harness fails on any silent overwrite.
- Of two conflicting decisions, the stale one is rejected and the rejection is recorded.
- A network handover does not replay a prompt or a decision.
- Reject stale, unauthorized, missing or conflicting input: each row asserts its own rejection path.

## Dependencies
CW-Q01b, CW-Q02a, CW-I05, CW-G05b, CW-N04, CW-N07. Owner gate CW-M03 (Gate M) is mandatory and is satisfied transitively through CW-I05 and CW-G05b. Owner gate CW-D05 is satisfied transitively.

## Validation
- Android reconnect rows on the physical device. Emulator results are not counted.
- Real-network revoke with the desktop on Ubuntu and the phone on the physical Android device.
- No check has been run for this planned item.

## Documentation impact
- [Testing and acceptance](../testing-and-acceptance.md): the device rows of the matrix and their invariants.
- [Consistency and recovery](../consistency-and-recovery.md): mapping from device invariants to rows.
