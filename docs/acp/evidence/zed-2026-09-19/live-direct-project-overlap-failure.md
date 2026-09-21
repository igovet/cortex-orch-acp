# Direct worker and coordinator turn overlap — 2026-09-19

Status: **FAIL** for end-to-end completion on source identity
`1.0.0+sha256.a21e964727423ff0`. This is a sanitized account of the
disposable Zed 1.20.2 session; screenshots remain outside the repository
because they contain account information.

The isolated Zed session opened
`/tmp/cortex-acp-live-ypkwriti/final/direct-mode-project-20260919`.
The starting unit test failed with `7 != 6`. A real Codex Luna high worker
ran with that exact directory as its working directory, changed `counter.py`
to return `sum(values)`, and published immutable report
`r_3757b86f2c5d`. The independent command
`PYTHONDONTWRITEBYTECODE=1 python3 -B -m unittest -v test_counter_unittest.py`
then passed in the Zed project.

The coordinator recorded acceptance of the worker result, but the runtime
blocked task `t_8d39a26ca283` as `coordinator_turn_mutated_workspace`.
It had compared the project tree before and after the coordinator's prompt;
the concurrently launched direct worker changed the same tree during that
prompt. No worker publication revision was recorded in time for the guard
to distinguish the observed worker result. The task was stopped through
the disposable Zed UI; its owned engine processes ended. The project
change and test pass do not convert the blocked task into a successful run.

The repair records a tree observation inside the worker's `publish_result`
MCP dispatch before exposing its result event and checks the exact observed
tree against the end of the coordinator turn. A new task in a fresh
disposable project is required to qualify that repair.
