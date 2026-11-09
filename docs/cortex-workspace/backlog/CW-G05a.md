# CW-G05a: Desktop Git panel

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Build the desktop Solid Git panel: status, diff, stage and unstage, commit preview, branch switch, fetch and pull, and push confirmation. Each step shows worktree identity and its typed outcome. No control turns a result acceptance into a commit or a push.

## Requirements
- CW-R11: status, diff, stage, commit, branch, pull and push, with explicit conflicts and no force push.
- CW-R05: result acceptance stays separate from Git actions.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): show exact worktree identity and changes before mutation; confirm the remote and branch before push.
- [ADR 0007](../adr/0007-result-acceptance.md) and [ADR 0009](../adr/0009-push.md).
- [Screens](../SCREENS.md) (desktop Git) and the projections and commands of [CW-G01](CW-G01.md) to [CW-G04](CW-G04.md).
- [CW-U02](CW-U02.md) for the task context in which the panel is reached.

## Deliverables
- Panel sections: changes, commit, branches, remote sync.
- Confirmation dialogs for commit, merge pull and push, each showing the exact preview.
- Conflict view listing unmerged files.
- Keyboard navigation through sections and file lists.

## Acceptance
Positive:
- On a clean fixture repository, stage, commit and push-confirm run end to end. Each step displays its typed outcome.
- The worktree path shown equals the path the runtime reports.

Negative:
- Accepting a Cortex result in the decision queue leaves the Git state unchanged. Repository hashes are compared before and after.
- The push control stays disabled until a current confirmation preview exists.
- No force-push control exists in the DOM.
- A dirty branch switch shows the refusal with its file list.

## Dependencies
- [CW-G04](CW-G04.md), [CW-U02](CW-U02.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Rendered run in the Tauri shell at 1280×800 and 1920×1080 against the fixture repository.
- Automated DOM assertions for the absence of force-push and acceptance-to-Git controls.
- Screenshots of the confirmation dialogs and the conflict view.
- Record exact commands, environment and outcomes.

## Documentation impact
- [Screens](../SCREENS.md) and [IDE and Git](../ide-and-git.md) when the panel is qualified.
- [Roadmap](../roadmap.md) when qualification changes.

## Size
L: a full desktop Git surface with confirmations, conflict views and negative DOM assertions.
