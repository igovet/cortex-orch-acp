# CW-Q06a: Ubuntu pilot checklist and desktop qualified-support statement

**Status:** planned · **Phase:** 3+ · **Size:** M

Split from [CW-Q06](CW-Q06.md) for the Ubuntu desktop release. The Android pilot is in [CW-Q06b](CW-Q06b.md).

## Goal
Give the owner a desktop pilot checklist for the Ubuntu release and a qualified-support statement for the desktop. Each qualified claim links to evidence rows. Each limitation is stated plainly. Anything without evidence is marked unqualified.

## Requirements
CW-R12 (Ubuntu is the first qualified platform; later platforms need their own qualification) and CW-R14 (measured evidence). See [requirements](../requirements.md).

## Inputs
- [Platform policy](../platforms.md): the first release is the Ubuntu desktop; Android is qualified separately and follows later.
- [Risks and open decisions](../risks-and-open-decisions.md): known limits of the first release.
- Evidence from CW-Q01a, CW-Q02a, CW-Q03a, CW-Q04a and CW-Q05.
- Gate D record: [desktop approval](../design/desktop-approval.md).

## Deliverables
- A pilot checklist for the owner's Ubuntu machine: clean install of the `.deb`, project registration, Cortex activation, description-only draft, start, observed progress, question answer, result reading, acceptance, and the update path from CW-U16 once a `.deb` exists.
- A qualified-support statement for the Ubuntu desktop. It lists the exact OS, desktop build and package versions, and states the evidence location for each claim.
- A limitation list: WebKitGTK GPU and session behavior depends on hardware (CW-F05); the desktop is the only qualified platform; ACP child process confinement is not claimed (CW-Q04a); and remote machines, relay and push are outside the first release.

## Acceptance
**Positive**
- Every qualified claim links to an executed evidence row.

**Negative**
- Any claim without evidence is marked unqualified.
- No statement implies universal connectivity or guaranteed push delivery.
- macOS, Windows and iOS are not described as supported. Android is not described as qualified in the desktop statement.
- Reject stale, unauthorized, missing or conflicting input: a checklist item without its evidence row is incomplete, not passed.

## Dependencies
CW-Q03a, CW-Q04a, CW-Q05. Owner gate CW-D05 is mandatory and is satisfied transitively. This item does not depend on any mobile item.

## Validation
- Local link and dependency scan of the checklist and support statement.
- Owner sign-off is recorded by the owner; it is not inferred.
- No check has been run for this planned item.

## Documentation impact
- A qualified-support page in the workspace documentation bundle, to be created with the implementation. It is outside this item's declared scope.
- [Platform policy](../platforms.md): desktop qualification status.
