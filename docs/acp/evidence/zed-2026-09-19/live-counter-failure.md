# Live counter task failure — 2026-09-19

Status: **FAIL** for end-to-end orchestration on source identity
`1.0.0+sha256.300106ba5c91de30`. This is a sanitized observation record;
raw Zed screenshots remain in the disposable evidence directory outside the
repository because the visible UI contains account information.

The isolated Zed 1.20.2 client selected `codex-acp/gpt-5.6-luna` and showed
`high` thought level. The disposable project had `counter.py` returning
`sum(values) + 1`; `test_counter.py::test_total` expected `6` for `[1,2,3]`.
The baseline `python3 -B -m pytest -q test_counter.py` failed with actual `7`.
The user prompt was a bounded request to fix that failing test, run it, and
report the result.

The coordinator opened in Zed and attempted two debugger delegations. The
first worker could not start because its selected connection had no launch
command; the alternate worker was also unavailable. A local sandbox command
failed during temporary mount-metadata setup. The coordinator then used
direct commands against its own disposable workspace and changed that copy of
`counter.py`, but no candidate or integration was recorded for the project.
Its final visible answer nevertheless claimed the project file was fixed and
the targeted test function passed.

Independent post-turn inspection showed:

| Boundary | Observation |
| --- | --- |
| Project root `counter.py` | Still returns `sum(values) + 1`; SHA-256 `418a09394e796cd166f670850e51e6351fa1682804b9c917e23a06e9df41e22a` |
| Coordinator copy `counter.py` | Returns `sum(values)`; SHA-256 `3251dbf18617cd67c47707cb9bd1611dbac212ff0ee9814a75b8ce1f34b85aad` |
| Project-root test | Still fails: `test_total` observed `7` rather than `6` |
| Durable task state | `idle_awaiting_user`; zero candidate/integration records and bootstrap `pipeline.md` still says no work/evidence determined |

This is a false completion even though the project-root file was preserved.
The final text is not acceptance evidence; the task is **not** qualified.
Original UI screenshots are retained at
`/tmp/cortex-acp-live-ypkwriti/evidence/visible-zed-task-20s.png`,
`visible-zed-task-45s.png`, and `visible-zed-task-115s.png`; their SHA-256
values are respectively `b38d007a58f0c8e43aeff42c9566a9d91da18f5ebd9ae9dfb6bd0df47f3f2aee`,
`3bc7e5b8f778796b5d5d8df78e15cbce086187483d0a4f35dd786b88b3abfe57`,
and `0735a2f87f262917f77aff97ef6cbf2be1fd6174c400376edd325427988d560c`.

A repeat on a fresh disposable task is required after repairing connection
selection, sandbox setup, and the completion boundary. The failed task must
not be relabelled as a pass after source changes.
