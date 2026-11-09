# CW-F04: Validate the current Cortex runtime's actual snapshot/candidate workspace paths and attempt lifecycle against source, as the concrete input to the Rust service's file/Git/PTY ownership boundary

**Status:** planned · **Phase:** 3+ · **Size:** L

## Goal

Produce a source-cited map of where the current runtime places each attempt's workspace copies, candidate stages and the main project root, and of the attempt lifecycle that governs them. Resolve the bundle's own snapshot/candidate contradiction against current source. Give the written sign-off that CW-I02 (the human mutation lease) will rely only on guarantees the runtime actually provides. This is the concrete input to the Rust service's file, Git and PTY ownership boundary.

## Requirements

CW-R06 (the machine service owns child sessions and reconciles crashes), CW-R10 (protect concurrent human and agent changes) and CW-R16 (restrict project paths and protect identities). See [requirements](../requirements.md).

## Inputs

Source-verified facts from the first read of this item (to be re-checked and cited with line numbers in the deliverable):

- `src/cortex_runtime/delegation/workspace_isolation.py`: the module docstring (lines 1-42) describes two owner-private copies per attempt, `base/` and `work/`. `_state_root()` (lines 335-339) resolves them under `<project>/.cortex/workspaces/`. `_EXCLUDED_TOP_LEVEL` (line 63) excludes top-level `.git` and `.cortex` from the copies.
- `src/cortex_runtime/delegation/candidates.py`: the module docstring (lines 1-6) describes a materialized candidate copy of the project plus selected attempt changes, independent of integration. The `candidate_staging` table (lines 25-46) records `stage_path`, `project_base_digest`, `content_digest` and `conflict_paths_json`. The directory the `stage_path` values point to is not yet verified.
- [workspaces-and-integration](../../features/workspaces-and-integration/index.md) introduction (lines 6-7) says "There are no hidden candidate copies". Its own candidate section (lines 154-161), and the note at lines 83-86, state that `candidates.py` implements candidate staging and that the conflict is open. The introduction is therefore stale.
- The same page (lines 57-59) states that external editors can change project files and that digests detect drift; a digest does not prevent writes.
- [storage](../../project/storage.md) (lines 34-37): changing work uses an isolated workspace and a candidate snapshot; integration requires the exact candidate digest and the expected base revision.

## Deliverables

- Path-class table: base copy, work copy, candidate stage, main project root, `.cortex` store, and any other path class found. For each: the template, the source file and symbol, and the lifecycle state that creates and removes it.
- Lifecycle map from attempt start through worker finish, candidate preparation, integration and cleanup, with the operation that performs each transition and the recorded state for each.
- Resolution of the contradiction: a correction to the introduction of [workspaces-and-integration](../../features/workspaces-and-integration/index.md), made in this item's documentation change, which removes the "no hidden candidate copies" claim.
- Written sign-off section for CW-I02 listing exactly what the lease model may assume: runtime-known writer processes, hash and digest checks, and the runtime's own operation lock. It must state that no OS-level isolation of the main tree from human or external editors exists.

## Acceptance

Positive:

- Every path class has a source citation (file and symbol, with line range) and a verified path template. Any path not confirmed in source is marked unverified, not inferred.
- The contradiction is marked resolved with the source citations that resolve it.
- The sign-off states the assumptions the lease model may and may not make, each tied to a cited source fact.

Negative:

- Any claim that worker copies or candidate stages isolate the main project tree from a human or external editor is rejected.
- A stale sentence claiming "no hidden candidate copies" still present in documentation when the sign-off is written means the item is not complete.
- A test-only or mock path is never cited as evidence of production layout.
- A missing source citation for any claim makes that claim unverified, and it cannot support the sign-off.

## Dependencies

CW-D05 (Gate D). Gate M does not gate this item.

## Validation

- `path-template-check`: a planned executable check that creates an attempt copy in a temporary project and asserts the documented path template; it extends the existing workspace isolation tests (`tests/test_workspace_isolation.py`), which are a baseline and not proof of this mapping.
- `citation-review`: each cited line range is re-read and confirmed before the sign-off.
- `contradiction-scan`: a repository-native search for the stale sentence, with the result recorded.

## Documentation impact

- [workspaces-and-integration](../../features/workspaces-and-integration/index.md): correct the stale introduction and mark the candidate-section conflict resolved.
- [architecture](../architecture.md) and [ide-and-git](../ide-and-git.md): replace the open "exact workspace ownership must be established" wording with the verified mapping.
- [risks](../risks-and-open-decisions.md): update the workspace-isolation entry with the sign-off outcome.
