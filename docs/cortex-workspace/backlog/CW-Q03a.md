# CW-Q03a: Rendered Ubuntu accessibility and long-history checks

**Status:** planned · **Phase:** 3+ · **Size:** L

Split from [CW-Q03](CW-Q03.md) for the desktop (Ubuntu) hardware.

## Goal
On the reference Ubuntu machine, verify that the rendered desktop app passes keyboard and assistive-technology checks. Measure the performance budgets for a 500-task board and a 10,000-event history.

## Requirements
CW-R12 (Ubuntu is a first qualified platform) and CW-R14 (responsiveness and accessibility measured against budgets). See [requirements](../requirements.md).

## Inputs
- [Performance targets](../performance.md): cold board, keystroke to glyph, scroll and drag, idle memory, 10,000 events and terminal budgets.
- [Platform policy](../platforms.md): WebKitGTK GPU and session behavior must be measured on representative hardware.
- CW-I04a, CW-I04b and CW-I04c (desktop editor and language servers) and CW-Q02a (desktop fault and race matrix, which exercises the same build).

## Deliverables
- An accessibility checklist: keyboard-only path through the board, the decision queue and the result screen; visible focus order; contrast; and accessible names in the AT-SPI tree.
- A performance harness recording cold start, keystroke-to-glyph latency, scroll frame timing, idle RSS, and the node count and frame rate for a 10,000-event history.
- Hardware, OS, GPU, WebKitGTK version, build identifier and dataset recorded alongside each budget.

## Acceptance
**Positive**
- Each budget row has a measured value and a pass or fail result against its target.
- The long-history view keeps DOM node count bounded to the viewport. The count is recorded.

**Negative**
- A budget that cannot be measured is reported as not measured. It is not counted as a pass.
- A budget miss blocks the performance claim.
- A keyboard trap or an unlabelled control fails its checklist row.
- Reject stale, unauthorized, missing or conflicting input: a missing instrument produces no pass.

## Dependencies
CW-Q02a, CW-I04a, CW-I04b, CW-I04c. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Rendered run on the reference Ubuntu machine. Record the desktop session, GPU and driver.
- AT-SPI inspection and keyboard-only walkthrough, with screenshots.
- No check has been run for this planned item.

## Documentation impact
- [Performance targets](../performance.md): measurement method and recorded hardware.
- [Testing and acceptance](../testing-and-acceptance.md): rendered Ubuntu qualification record.
