# CW-G01: Git read projection through git2-rs

**Status:** planned · **Phase:** 3+ · **Size:** M

## Goal
Provide the status, diff, log and blame projections for the registered worktree, in process through git2-rs. This replaces per-call Git CLI spawns on read paths. The reported worktree identity must equal the path the runtime reports.

## Requirements
- CW-R11: status and diff are available, with explicit identity before any mutation.
- CW-R10: Git read surfaces stay bounded and path-checked.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): git2-rs for status, diff, log and index operations; the Git CLI for commit, branch, fetch, pull and push. Show exact worktree identity and changes before mutation.
- Root registration from [CW-I01](CW-I01.md) and the worktree-identity source to be validated in [CW-F04](CW-F04.md).
- [Performance](../performance.md) lists a proposed Git read budget, pending owner acceptance and measurement. The 100 ms warm-latency target below is that proposal.

## Deliverables
- Status, diff, log and blame projections with paging.
- Worktree identity field (path, HEAD, branch or detached state), checked against the runtime-reported path.
- Fixture repositories: a 5,000-commit history, a linked worktree, a detached HEAD, and a repository with an unreadable object.

## Acceptance
Positive:
- On the 5,000-commit fixture, warm status, diff and log each return within 100 ms (proposed budget) on recorded hardware, with zero Git subprocess spawns during the call loop.
- The worktree identity for the linked worktree matches the runtime-reported path.

Negative:
- A worktree-identity mismatch is refused. It is never reported as success.
- A corrupt or unreadable repository returns a typed error, never an empty status.
- Read projections do not change refs or the index file. File hashes are compared before and after.
- A path outside the registered worktree is rejected.

## Dependencies
- [CW-I01](CW-I01.md) (root registration and path rules).
- Gate records: CW-D05 (Gate D).

## Validation
- `cargo test` with a generated 5,000-commit repository (`git fast-import`).
- Subprocess counting during the call loop, for example `strace -f -e trace=execve`.
- Timed warm calls on recorded hardware; compare `.git` file hashes before and after.
- Record exact commands, toolchain and outcomes.

## Documentation impact
- [Performance](../performance.md): add the Git read row once measured.
- [Architecture](../architecture.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
M: read-only projections over one library, with a specific subprocess and identity contract.
