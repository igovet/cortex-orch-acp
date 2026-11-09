# CW-U04: Versioned result screen on the CodeMirror merge view

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Build a read-only result screen for one specific result version. It shows the exact revision digest, the evidence list (passed, failed, unrun, unknown, missing), and the "what changed" view using @codemirror/merge in unified and split modes. Acceptance is a decision in [CW-U03](CW-U03.md), referenced from this screen, not a control inside it. The merge view is to be validated by CW-F03; this item does not assume that validation has already happened.

## Requirements
- CW-R05: evidence, missing checks and the exact revision are shown; acceptance stays separate from Git actions.
- CW-R03: result acceptance is a distinct decision kind.

See [requirements](../requirements.md).

## Inputs
- [Screens](../SCREENS.md): result and diff must distinguish unrun from passed checks; unknown status is never styled as success.
- [ADR 0007](../adr/0007-result-acceptance.md) and [IDE and Git](../ide-and-git.md): accepting a result does not commit or push.
- CW-C05 (digest-current projection), CW-C07 (version-bound acceptance).
- The CodeMirror merge-view fixture in [CW-F03](CW-F03.md).

## Deliverables
- Header: result digest, workspace identity, and the acceptance state for this digest (not accepted, accepted for digest X, or invalidated because the digest changed).
- Evidence panel with labelled states that use text and an icon, never color alone.
- Diff view in unified and split modes, read-only, with keyboard navigation through files and hunks.
- A link to the acceptance decision in CW-U03.
- Check recovery ([CW-R40](../requirements.md)): a skipped check runs automatically; a failed or not-run check offers a forced re-run. Acceptance with an unpassed check is offered only when that check cannot complete for a technical reason, and it needs an explicit confirmation.

## Acceptance
Positive:
- A known fixture diff renders in unified and split modes with the same file count, hunk count and changed-line count as the fixture manifest.
- The header digest matches the digest referenced by the acceptance decision.

Negative:
- Accept and reject-chunk controls are absent in both modes (checked in the DOM).
- After the digest has changed following an acceptance, the screen shows "acceptance invalidated by a newer digest", never "accepted".
- A check recorded as unrun or unknown never renders with pass styling or a checkmark.
- Missing evidence is shown as missing. The screen does not infer a pass from absent checks.

## Dependencies
- [CW-U02](CW-U02.md) (task context), CW-C07 (version-bound acceptance), [CW-F03](CW-F03.md) (merge-view validation).
- Gate records: CW-D05 (Gate D).

## Validation
- Render the fixture diff at 1280×800 in both modes; capture screenshots.
- Automated DOM assertions for the absence of accept and reject-chunk controls.
- Digest-change fixture: confirm the invalidated state is displayed.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Screens](../SCREENS.md): result-screen states.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
M: one read-only screen with a merge-view component to be validated in CW-F03. A bespoke diff component is out of scope.
