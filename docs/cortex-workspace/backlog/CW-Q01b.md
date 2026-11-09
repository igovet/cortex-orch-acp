# CW-Q01b: Phone leg of the fake-ACP scenario

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q01](CW-Q01.md) for the phone. This is a mobile qualification item and is gated by Gate M.

## Goal
Extend the [CW-Q01a](CW-Q01a.md) desktop scenario with a paired phone. The phone receives the question, answers it, and the desktop then shows the accepted result with its evidence. The transcript is the one used by CW-Q01a, with the phone steps added.

## Requirements
CW-R04 (stale decision versions are rejected), CW-R12 (Android qualification) and CW-R14 (measured responsiveness and accessibility, not claims). See [requirements](../requirements.md).

## Inputs
- [CW-Q01a](CW-Q01a.md): scenario runner, transcript and step record.
- CW-A03 (Android result and acceptance screens) and CW-N03 (QR/link enrollment and pairing).
- CW-H04 (crash and reconciliation) and CW-U04 (desktop result screen, where acceptance is shown).

## Deliverables
- Phone steps added to the transcript: question on phone → answer on phone → result shown read-only on phone → accept on desktop.
- A test pairing harness or a physically paired phone. The runner uses the pairing from CW-N03.
- A step record for the phone steps, with versions and event IDs.

## Acceptance
**Positive**
- The phone leg completes. The desktop shows the accepted result with its evidence.
- Two runs produce the same event sequence, ignoring timestamps.

**Negative**
- A phone answer carrying a stale decision version is rejected. The scenario records the rejection, and the desktop state is not changed by it.
- The runner rejects an unpaired phone.
- A missing transcript step fails the scenario. No success is inferred from absent evidence.
- Changing the result digest after review invalidates acceptance. The scenario includes this step.

## Dependencies
CW-Q01a, CW-A03, CW-H04, CW-U04. Owner gate CW-M03 (Gate M) is mandatory and is satisfied transitively through CW-A03. Owner gate CW-D05 is satisfied transitively.

## Validation
- Kotlin instrumented test on the physical Android device, driven by the runner. Emulator results are not counted.
- Rust integration test driving the machine service with the fake agent.
- No check has been run for this planned item.

## Documentation impact
- [Testing and acceptance](../testing-and-acceptance.md): phone leg step list.
- [Roadmap](../roadmap.md): mobile qualification in M6; no status change until qualification.
