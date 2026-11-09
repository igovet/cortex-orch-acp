# CW-G02: Version-checked stage and unstage, with CLI-backed commit

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal
Stage and unstage through the git2-rs index API. Each operation is guarded by the expected file state hash. Commit preview and commit execution go through the installed Git CLI, so the owner's hooks, signing and credential helpers run unchanged.

## Requirements
- CW-R11: stage and commit are available with explicit identity and changes shown before mutation.
- CW-R04: each mutation carries an expected version and is idempotent.

See [requirements](../requirements.md).

## Inputs
- [IDE and Git](../ide-and-git.md): index operations in git2-rs; commit through the Git CLI so hooks and signing behave as the owner's configuration expects.
- [CW-G01](CW-G01.md) (projections and worktree identity) and [CW-I02](CW-I02.md) (writer quiescence before index writes).
- [ADR 0006](../adr/0006-human-edit-git.md).

## Deliverables
- Stage and unstage commands, each with an expected per-file hash. A changed file returns a conflict result.
- Commit preview: staged files, message, configured author identity and worktree identity.
- Commit execution with `git commit` through the CLI. The command builder never adds flags that bypass hooks. The result reads the new commit identifier back from HEAD.
- Typed failure for a rejected hook, including the hook output.

## Acceptance
Positive:
- A stage then unstage round trip changes only the index and spawns no subprocess.
- A commit on a fixture with a pre-commit hook runs the hook and creates a commit. With a test signing key generated inside the test directory, signature verification passes.

Negative:
- Staging against a changed file hash is refused, and the index is unchanged.
- A commit with an empty staged set is refused.
- A hook rejection is reported as failed with its output. It is not retried.
- The argument list never contains `--no-verify` or an equivalent bypass flag (argv audit).
- The item never sets a signing key or identity. It uses the owner's Git configuration.

## Dependencies
- [CW-G01](CW-G01.md), [CW-I02](CW-I02.md).
- Gate records: CW-D05 (Gate D).

## Validation
- Rust integration tests on Ubuntu with a fixture repository and a pre-commit hook.
- Test GPG key generated in a temporary directory. Owner keys are never used.
- Argv audit log for every commit call in the test run.
- Record exact commands, Git version and outcomes.

## Documentation impact
- [IDE and Git](../ide-and-git.md) and [roadmap](../roadmap.md) when qualification changes.

## Size
L: index mutation with version checks, CLI commit with hook and signing behaviour, and an argv audit.
