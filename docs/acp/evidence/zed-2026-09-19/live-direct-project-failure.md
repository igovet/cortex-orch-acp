# Direct-project Zed task failure — 2026-09-19

Status: **FAIL** for end-to-end orchestration on source identity
`1.0.0+sha256.5093364f217ff415`. This is a sanitized observation record;
the original Zed screenshots remain outside the repository because the UI
contains account information.

The isolated, visible Zed 1.20.2 session opened
`/tmp/cortex-acp-live-ypkwriti/project`. The task asked an implementation
worker to fix `counter.py` in that exact directory and run
`python3 -B -m unittest -v test_counter_unittest.py`. Before the prompt, that
test failed with `7 != 6`.

The coordinator first delegated `backend_dev` through
`claude-agent-acp/sonnet/medium`, a route permitted by the project policy but
not connected in this isolated Zed profile. The assignment failed with
`engine_not_connected`. The coordinator then requested
`codex-acp/gpt-5.6-luna/high`; that worker reached `assignment_ready` and
began inspecting the project. The user stopped the test at the first failed
assignment. No further live prompt was requested. The task was marked
`stopped`, and its owned engine processes were terminated.

The independent project-root test still fails with `7 != 6`; `counter.py`
remains unchanged (SHA-256
`418a09394e796cd166f670850e51e6351fa1682804b9c917e23a06e9df41e22a`).
The direct worker did run with the Zed project as its working directory, but
the engine created a `codex-bwrap-synthetic-mount-targets-1000` temporary
directory there. That is project pollution, not a task result. The current
run also showed that stopping the task did not automatically terminate one
coordinator engine process; that exact owned process was terminated after
the task reached `stopped`.

This run must not be counted as success. A later run requires connection-aware
route advertisement, an engine temporary directory outside the project, and
verification that stopping the task cleans up every owned process. The
previous failed run remains separately recorded in
[live-counter-failure.md](live-counter-failure.md).
