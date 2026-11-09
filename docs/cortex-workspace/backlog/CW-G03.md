# CW-G03: Branch switch, fetch and pull through the Git CLI

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Create and switch branches, fetch and pull through the Git CLI, spawned with tokio::process. Unsafe operations are refused when the worktree is dirty. Merge conflicts stay visible as typed state. The service never stashes, resets or cleans on the owner's behalf.

## Requirements
- CW-R11: branch, fetch and pull are available with explicit conflicts; no force push and no hidden reset.
- CW-R07: an unknown outcome remains unknown until it is checked.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): dirty branch changes and merge conflicts remain visible; never auto-stash or reset.
- [CW-G02](CW-G02.md) (index state and commit path).
- [Consistency and recovery](../consistency-and-recovery.md): unknown external outcomes are not replayed.

## Deliverables
- Branch list projection from git2 reads; create and switch commands through the CLI.
- Pre-checks: a dirty worktree is listed by file and blocks the switch.
- Fetch and pull. Pull is fast-forward only by default. A merge pull requires explicit confirmation.
- Conflict projection built from unmerged index entries.

## Acceptance
Positive:
- Switching on a clean fixture updates HEAD, and the projection reflects it after one refresh.
- Fetching from a local fixture remote updates the remote-tracking refs and the projection.

Negative:
- A switch on a dirty worktree is refused with the file list. No stash or reset runs.
- A pull that conflicts leaves conflict markers and unmerged entries visible. It is not auto-aborted or auto-resolved.
- A fetch whose transport fails mid-operation is shown as unknown and is not retried automatically.
- The argv audit finds no `stash`, `reset --hard`, `checkout -f` or `clean` command.

## Dependencies
- [CW-G02](CW-G02.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Fixture repositories: two clones that produce conflicting edits.
- Argv audit over the whole test run.
- Fault injection in fetch (dropped connection).
- Record exact commands, Git version and outcomes.

## Documentation impact
- [IDE and Git](../ide-and-git.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
L: branch and remote operations, conflict projection and refusals that must hold under every dirty-state path.
